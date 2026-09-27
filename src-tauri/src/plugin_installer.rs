//! Installing, updating and removing plugin packages.
//!
//! A plugin arrives as a signed `.orivo-plugin` archive. The host reads it
//! wholly in memory, re-derives every digest, checks the package against the
//! v1 policy in [`crate::plugin_manifest`], asks the runtime whether the
//! component can actually run — and only then hands it to
//! [`crate::plugin_update`], which makes it the live version through a
//! transaction that survives being killed at any step.
//!
//! Two channels, two signature policies. A package pulled from the registry
//! must carry a signature made by Orivo's own key; it may be updated
//! automatically once the user has consented once. A package the user picks by
//! hand may be unsigned; it installs as `Development`, every surface that shows
//! it says so, and nothing updates it but the user.
//!
//! Where the checks live is deliberate. The *package* is judged before anything
//! is written: format, digests, signature, ABI, and — new here — whether a
//! runner component really implements the runner contract, which is the gap
//! that let a package install and then appear as a broken row. The *plugin* is
//! judged again after the swap, at its final path, because that is the only
//! place the identity check discovery runs can be run: a staged directory is
//! not named after the plugin. A refusal there rolls the update back.

use crate::plugin_index::{
    IndexCache, IndexEntry, REGISTRY_INDEX_URL, download_entry, is_upgrade, refresh_index,
};
use crate::plugin_manifest::{
    HostCompatibility, PackageEntry, PackageInspection, PackageSignatureStatus, PluginManifest,
    ValidatedPluginPackage, validate_plugin_package,
};
use crate::plugin_registry::{
    PLUGINS_DIRECTORY, PackageRefusal, PluginRegistry, PluginState, VerifyDepth,
};
use crate::plugin_runtime::PluginRuntime;
use crate::plugin_update::{
    Checkpoint, IdentityObserver, PackageChannel, PackageFiles, PluginStore, RecoveryOutcome,
    valid_plugin_id,
};
use ed25519_dalek::{Signature, VerifyingKey};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Emitter, State};

const MAIN_WINDOW_LABEL: &str = "main";
pub const PLUGIN_INSTALL_EVENT: &str = "plugin-install-status";

const REGISTRY_JSON: &str = include_str!("../resources/plugin-registry.json");
const MANIFEST_FILE: &str = "manifest.json";
const COMPONENT_FILE: &str = "component.wasm";
const SIGNATURE_FILE: &str = "signature.ed25519";
/// Mirrors `MAX_PACKAGE_BYTES` in the manifest policy, with headroom for the
/// signature and the archive's own framing.
const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PACKAGE_ENTRIES: usize = 64;

const POLICY_FILE: &str = ".cache/update-policy.json";

/// Orivo's release signing key. A registry package that is not signed by this
/// key is refused: the registry is a distribution channel, not a trust
/// boundary the user is asked to evaluate per download.
///
/// The matching private key lives only in the plugin project's gitignored
/// `keys/` directory. Rotating it is a host release, which is the point: a
/// compromised signing key cannot be replaced by anything a package says.
const RELEASE_PUBLIC_KEY_BASE64: &str = "OX9NRNeAEL2tEyS54qUTJ14cFS6smfLu6JoPzbiXG9w=";

/// The name recorded beside a plugin that arrived release-signed. It is written
/// down rather than implied so that rotating the key above shows up in the
/// record as a different signer, instead of silently reinterpreting every
/// package already installed.
pub const ORIVO_RELEASE_SIGNER: &str = "orivo-release-v1";

/// How many times a smoke test that gave no answer is asked again before the
/// update is undone.
///
/// The realistic cause is contention, not a bad package: the scheduler runs one
/// job at a time per plugin, so a discovery page already in flight can push the
/// probe past its bounded wait. Three tries a quarter of a second apart outlast
/// that without turning a genuinely wedged plugin into a long stall — and a
/// failure after them still rolls back, it just says which kind of failure it
/// was.
const SMOKE_TEST_ATTEMPTS: usize = 3;
const SMOKE_TEST_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

// ---------------------------------------------------------------------------
// IPC views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub state: PluginState,
    pub message: String,
    pub trusted: bool,
    /// The version Orivo kept when this one was installed, if any. It is the
    /// only thing "Go back to the previous version" can mean, so it is a fact
    /// about disk rather than a button that might do nothing.
    pub rollback_to: Option<String>,
    /// Whether that version is signed by Orivo's release key — `None` when
    /// there is no rollback target at all. A rollback is allowed to put a
    /// development build back in place of a signed one (`plugin_update.rs`'s
    /// `grant_verdict`: it is a version the user had), so Settings needs this
    /// to warn before it does rather than after.
    pub rollback_trusted: Option<bool>,
    /// A newer release in the registry, from the cached index only. Resolving
    /// it never touches the network.
    pub update_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailablePluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub size_bytes: u64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogView {
    pub installed: Vec<InstalledPluginView>,
    pub available: Vec<AvailablePluginView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginUpdatePolicy {
    /// Off until the user says otherwise, once, for the whole official channel.
    /// A development build is never included whatever this says.
    pub automatic: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginInstallProgress {
    plugin_id: String,
    phase: &'static str,
    percent: u8,
    message: String,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PluginInstallerService {
    plugin_root: PathBuf,
    host_version: &'static str,
    store: PluginStore,
    index: IndexCache,
    /// One flag for the one network operation a user can see at a time. It is
    /// cleared when an operation starts, so `cancel_plugin_update` abandons
    /// what is running rather than poisoning what comes next.
    cancelled: AtomicBool,
}

impl PluginInstallerService {
    pub fn new(plugin_root: PathBuf, host_version: &'static str) -> Self {
        Self {
            store: PluginStore::new(plugin_root.clone()),
            index: IndexCache::new(&plugin_root),
            cancelled: AtomicBool::new(false),
            plugin_root,
            host_version,
        }
    }

    fn registry(&self) -> PluginRegistry {
        PluginRegistry::new(
            self.plugin_root.clone(),
            HostCompatibility::v1(self.host_version),
        )
    }

    /// Everything the registry offers this build: the list compiled into the
    /// binary, overlaid by the cached signed index. No network, ever — this is
    /// read on a path that renders.
    fn available_entries(&self) -> Vec<IndexEntry> {
        let mut entries = crate::plugin_index::parse_unsigned_entries(REGISTRY_JSON.as_bytes());
        if let Some(index) = self.index.load() {
            for entry in index.entries {
                match entries.iter_mut().find(|known| known.id == entry.id) {
                    Some(known) => *known = entry,
                    None => entries.push(entry),
                }
            }
        }
        entries.retain(|entry| self.host_can_run(entry));
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        entries
    }

    /// A release that needs a newer Orivo is not an update, it is a reason to
    /// update Orivo. Hiding it is better than offering an install that the
    /// manifest check would refuse after the download.
    fn host_can_run(&self, entry: &IndexEntry) -> bool {
        entry
            .min_orivo_version
            .as_deref()
            .is_none_or(|minimum| !is_upgrade(minimum, self.host_version))
    }

    fn installed(&self) -> Vec<InstalledPluginView> {
        // One process-wide runtime: its compiled-component cache, memory
        // ceiling and epoch ticker are only global if every surface shares them.
        let Ok(runtime) = PluginRuntime::shared() else {
            return Vec::new();
        };
        let available = self.available_entries();
        self.registry()
            .installed_plugins(&runtime)
            .into_iter()
            .map(|record| {
                let rollback_target = self.store.rollback_target(&record.id);
                InstalledPluginView {
                    trusted: self.store.is_trusted(&record.id),
                    rollback_to: rollback_target
                        .as_ref()
                        .map(|target| target.version.clone()),
                    rollback_trusted: rollback_target
                        .as_ref()
                        .map(|target| target.channel.is_official()),
                    update_to: available
                        .iter()
                        .find(|entry| entry.id == record.id)
                        .filter(|entry| is_upgrade(&entry.version, &record.version))
                        .map(|entry| entry.version.clone()),
                    id: record.id,
                    name: record.name,
                    version: record.version,
                    extensions: record.extension_names,
                    state: record.state,
                    message: record.message,
                }
            })
            .collect()
    }

    fn catalog(&self) -> PluginCatalogView {
        // Settings › Plugins is a user-initiated surface, so the compile cache may
        // be opened from here on. Nothing before this point in a session does.
        crate::plugin_compile_cache::permit();
        let installed = self.installed();
        let available = self
            .available_entries()
            .into_iter()
            .map(|entry| AvailablePluginView {
                installed: installed.iter().any(|plugin| plugin.id == entry.id),
                id: entry.id,
                name: entry.name,
                version: entry.version,
                summary: entry.summary,
                size_bytes: entry.size_bytes,
            })
            .collect();
        PluginCatalogView {
            installed,
            available,
        }
    }

    /// Settle anything a previous run was in the middle of. Called before the
    /// first read and before every write, because a journal left by a crash
    /// must never be overwritten by the next transaction.
    pub fn recover_interrupted_updates(&self) -> Vec<RecoveryOutcome> {
        self.store.recover()
    }

    /// Remove a plugin and everything the host kept about it. The command is
    /// this, on a blocking worker.
    pub(crate) fn uninstall(&self, plugin_id: &str) -> Result<(), String> {
        self.store.remove(plugin_id)
    }

    /// Go back to the version Orivo kept, returning which one that was.
    pub(crate) fn roll_back(&self, plugin_id: &str) -> Result<String, String> {
        self.store.rollback(plugin_id).map(|target| target.version)
    }

    /// Be told when an installed plugin becomes a different package, or stops
    /// being installed.
    ///
    /// Registered from `lib.rs` so the installer never has to know who is
    /// listening. The consumer this exists for is the one the plan names next:
    /// a capability grant, and a runner profile marked valid, are agreements
    /// with a *package* — the component behind the id — so they have to be
    /// re-asked for when that package changes and dropped when it goes away.
    pub fn observe_identity(&self, observer: IdentityObserver) {
        self.store.observe(observer);
    }

    /// What the live version of a plugin is: its version, the SHA-256 of the
    /// component that will actually run, and which key signed it. The stable
    /// answer to "is this still the package that grant was given to".
    ///
    /// Unused in this build on purpose: it is the read half of the seam E2
    /// wires itself to once third-party runners have profiles to invalidate.
    /// Landing it with the write half is the point — an interface that arrives
    /// after the code meant to consume it is one nobody designed against.
    #[allow(dead_code)]
    pub fn package_identity(
        &self,
        plugin_id: &str,
    ) -> Option<crate::plugin_update::PackageIdentity> {
        self.store.identity(plugin_id)
    }

    fn policy_path(&self) -> PathBuf {
        self.plugin_root.join(POLICY_FILE)
    }

    fn policy(&self) -> PluginUpdatePolicy {
        fs::read(self.policy_path())
            .ok()
            .filter(|bytes| bytes.len() < 4096)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(PluginUpdatePolicy { automatic: false })
    }

    fn set_policy(&self, policy: &PluginUpdatePolicy) -> Result<(), String> {
        let path = self.policy_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| "The plugin folder is unavailable.".to_string())?;
        }
        let encoded = serde_json::to_vec(policy)
            .map_err(|_| "That setting could not be saved.".to_string())?;
        fs::write(&path, encoded).map_err(|_| "That setting could not be saved.".to_string())
    }

    fn begin_network_operation(&self) {
        self.cancelled.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_plugin_catalog(
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<PluginCatalogView, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.catalog())
        .await
        .map_err(|_| "The plugin catalogue could not be read.".to_string())
}

/// Ask the registry what it has. Separate from [`get_plugin_catalog`] on
/// purpose: the catalogue renders, and a path that renders never waits on a
/// socket. Cancellable, because a user who closes the panel has answered.
#[tauri::command]
pub async fn refresh_plugin_registry(
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<PluginCatalogView, String> {
    let service = Arc::clone(&service);
    service.begin_network_operation();
    // A registry that cannot be reached is not an error the catalogue has to
    // carry: the compiled-in list and the last cached index are still the
    // truth, and saying so would turn a flaky network into a broken Store.
    let _ = refresh_index(&service.index, REGISTRY_INDEX_URL, &service.cancelled).await;
    tauri::async_runtime::spawn_blocking(move || service.catalog())
        .await
        .map_err(|_| "The plugin catalogue could not be read.".to_string())
}

#[tauri::command]
pub fn cancel_plugin_update(service: State<'_, Arc<PluginInstallerService>>) {
    service.cancelled.store(true, Ordering::Release);
}

#[tauri::command]
pub async fn get_plugin_update_policy(
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<PluginUpdatePolicy, String> {
    Ok(service.policy())
}

#[tauri::command]
pub async fn set_plugin_update_policy(
    automatic: bool,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<PluginUpdatePolicy, String> {
    let policy = PluginUpdatePolicy { automatic };
    service.set_policy(&policy)?;
    Ok(policy)
}

#[tauri::command]
pub async fn install_plugin_from_registry(
    app: AppHandle,
    plugin_id: String,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<(), String> {
    let service = Arc::clone(&service);
    let entry = service
        .available_entries()
        .into_iter()
        .find(|entry| entry.id == plugin_id)
        .ok_or_else(|| "This plugin is not in Orivo's registry.".to_string())?;
    // Installing and updating are the same transaction, so they need the same
    // guard. Without it, "install" was the door a replayed registry entry could
    // walk an installed plugin backwards through, while "update" refused.
    refuse_a_downgrade(
        installed_version(&service, &plugin_id).as_deref(),
        &entry.version,
    )?;
    acquire_and_install(&app, &service, &entry).await
}

/// Replace an installed plugin with a newer release from the registry.
///
/// Refuses anything that is not strictly newer. A signature stays valid
/// forever, so "install whatever the registry names" would let a replayed
/// entry walk a plugin backwards onto a version whose bug is already known.
#[tauri::command]
pub async fn update_plugin(
    app: AppHandle,
    plugin_id: String,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<(), String> {
    let service = Arc::clone(&service);
    let entry = service
        .available_entries()
        .into_iter()
        .find(|entry| entry.id == plugin_id)
        .ok_or_else(|| "This plugin is not in Orivo's registry.".to_string())?;
    let installed = installed_version(&service, &plugin_id)
        .ok_or_else(|| "That plugin is not installed.".to_string())?;
    refuse_a_downgrade(Some(&installed), &entry.version)?;
    acquire_and_install(&app, &service, &entry).await
}

/// Go back to the version Orivo kept. The manual half of the same transaction
/// the smoke test runs automatically.
#[tauri::command]
pub async fn rollback_plugin(
    plugin_id: String,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<String, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.roll_back(&plugin_id))
        .await
        .map_err(|_| "The rollback did not finish.".to_string())?
}

/// Synchronous on purpose: macOS requires the native picker on the main
/// thread, which is where Tauri runs a non-async command. The work that
/// follows is bounded by `MAX_PACKAGE_BYTES` and reads an already-local file,
/// so it does not need to leave that thread — the same shape `import_game`
/// uses for picking an executable.
#[tauri::command]
pub fn install_plugin_from_file(
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        return Ok(None);
    }
    #[cfg(not(target_os = "android"))]
    {
        let Some(selected) = rfd::FileDialog::new()
            .set_title("Choose an Orivo plugin package")
            .add_filter("Orivo plugin", &["orivo-plugin"])
            .pick_file()
        else {
            return Ok(None);
        };
        let bytes = read_bounded_file(&selected, MAX_PACKAGE_BYTES)
            .map_err(|_| "This package could not be read.".to_string())?;
        // A package the user picked by hand may be unsigned. It installs as a
        // development build and every surface that lists it says so.
        install_package(&service, &bytes, SignaturePolicy::AllowUnsigned, None).map(Some)
    }
}

#[tauri::command]
pub async fn uninstall_plugin(
    plugin_id: String,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<(), String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.uninstall(&plugin_id))
        .await
        .map_err(|_| "The removal did not finish.".to_string())?
}

/// Everything the host does to the plugin folder without a user waiting for it:
/// settle an interrupted transaction, then — only with consent — look for
/// releases and take them.
///
/// Spawned rather than awaited at startup. The plan's first promise is that the
/// shell appears without a plugin, and a registry that is slow to answer must
/// not be able to delay it.
pub fn start_background_maintenance(app: AppHandle, service: Arc<PluginInstallerService>) {
    tauri::async_runtime::spawn(async move {
        let recovering = Arc::clone(&service);
        let _ =
            tauri::async_runtime::spawn_blocking(move || recovering.recover_interrupted_updates())
                .await;
        if !service.policy().automatic {
            return;
        }
        service.begin_network_operation();
        if refresh_index(&service.index, REGISTRY_INDEX_URL, &service.cancelled)
            .await
            .is_err()
        {
            return;
        }
        // Reading every installed manifest is disk work, and the command executor
        // is not where disk work goes: `spawn_blocking`, like the recovery step
        // above it.
        let listing = Arc::clone(&service);
        let Ok(pending) =
            tauri::async_runtime::spawn_blocking(move || pending_automatic_updates(&listing)).await
        else {
            return;
        };
        for entry in pending {
            if service.cancelled.load(Ordering::Acquire) {
                return;
            }
            let _ = acquire_and_install(&app, &service, &entry).await;
        }
    });
}

/// The releases consent covers: an installed plugin, on the official channel,
/// with a strictly newer entry in the registry.
///
/// A development build is absent by construction — it carries no trust marker,
/// and a sideloaded package silently becoming an official one is exactly the
/// impersonation the two channels exist to prevent.
fn pending_automatic_updates(service: &PluginInstallerService) -> Vec<IndexEntry> {
    let available = service.available_entries();
    // The manifest listing, not `installed()`: this runs at launch, and the three
    // fields it reads — id, version, whether the package arrived signed — are the
    // manifest's and the trust marker's. `installed()` would preflight every
    // installed component to answer them, which is a Cranelift pass per plugin on
    // the startup path and, with a compile cache behind it, the install key read
    // at launch. See `plugin_compile_cache::permit`.
    service
        .registry()
        .installed_manifests()
        .into_iter()
        .filter(|record| service.store.is_trusted(&record.id))
        .filter_map(|record| {
            available
                .iter()
                .find(|entry| entry.id == record.id)
                .filter(|entry| is_upgrade(&entry.version, &record.version))
                .cloned()
        })
        .collect()
}

async fn acquire_and_install(
    app: &AppHandle,
    service: &Arc<PluginInstallerService>,
    entry: &IndexEntry,
) -> Result<(), String> {
    service.begin_network_operation();
    publish(app, &entry.id, "downloading", 0, "Downloading…");
    let size = entry.size_bytes.max(1);
    let mut on_progress = |read: u64| {
        publish(
            app,
            &entry.id,
            "downloading",
            ((read as f64 / size as f64) * 100.0) as u8,
            "Downloading…",
        );
    };
    let bytes = match download_entry(entry, &service.cancelled, &mut on_progress).await {
        Ok(bytes) => bytes,
        Err(error) => {
            publish(app, &entry.id, "failed", 0, &error);
            return Err(error);
        }
    };

    publish(app, &entry.id, "verifying", 100, "Verifying…");
    let service_for_install = Arc::clone(service);
    let expected_id = entry.id.clone();
    let promise = PackagePromise {
        id: entry.id.clone(),
        version: entry.version.clone(),
    };
    let installed = tauri::async_runtime::spawn_blocking(move || {
        // The registry is a distribution channel, not a trust decision the
        // user is asked to make per download: a release signature is required,
        // and the package has to be the one the entry named.
        install_package(
            &service_for_install,
            &bytes,
            SignaturePolicy::ReleaseOnly,
            Some(&promise),
        )
    })
    .await
    .map_err(|_| "The installation did not finish.".to_string())?;

    match installed {
        Ok(id) => {
            publish(app, &id, "installed", 100, "Installed.");
            Ok(())
        }
        Err(error) => {
            publish(app, &expected_id, "failed", 0, &error);
            Err(error)
        }
    }
}

fn publish(app: &AppHandle, plugin_id: &str, phase: &'static str, percent: u8, message: &str) {
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        PLUGIN_INSTALL_EVENT,
        PluginInstallProgress {
            plugin_id: plugin_id.to_string(),
            phase,
            percent: percent.min(100),
            message: message.to_string(),
        },
    );
}

fn installed_version(service: &PluginInstallerService, plugin_id: &str) -> Option<String> {
    let bytes = read_bounded_file(
        &service.plugin_root.join(plugin_id).join(MANIFEST_FILE),
        64 * 1024,
    )
    .ok()?;
    let manifest = serde_json::from_slice::<PluginManifest>(&bytes).ok()?;
    Some(manifest.version)
}

// ---------------------------------------------------------------------------
// Package reading, validation and installation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignaturePolicy {
    /// Only a package signed by Orivo's release key is accepted.
    ReleaseOnly,
    /// An unsigned package is accepted and marked as a development build. A
    /// present-but-wrong signature is still a hard failure.
    AllowUnsigned,
}

/// What the registry promised. `None` for a package the user picked by hand:
/// there is no third party making a claim to hold it to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PackagePromise {
    id: String,
    version: String,
}

fn install_package(
    service: &PluginInstallerService,
    bytes: &[u8],
    policy: SignaturePolicy,
    expected: Option<&PackagePromise>,
) -> Result<String, String> {
    let files = read_package(bytes)?;
    let manifest_bytes = files
        .get(MANIFEST_FILE)
        .ok_or_else(|| "The package has no manifest.".to_string())?;
    let manifest = serde_json::from_slice::<PluginManifest>(manifest_bytes)
        .map_err(|_| "The package manifest is not valid JSON.".to_string())?;

    let signature = signature_status(manifest_bytes, files.get(SIGNATURE_FILE));
    let signature = match (policy, signature) {
        (SignaturePolicy::ReleaseOnly, PackageSignatureStatus::Trusted) => {
            PackageSignatureStatus::Trusted
        }
        (SignaturePolicy::ReleaseOnly, _) => {
            return Err("This package is not signed by Orivo.".into());
        }
        (SignaturePolicy::AllowUnsigned, PackageSignatureStatus::Invalid) => {
            return Err("The package signature is invalid.".into());
        }
        (SignaturePolicy::AllowUnsigned, PackageSignatureStatus::Missing) => {
            PackageSignatureStatus::Development
        }
        (SignaturePolicy::AllowUnsigned, status) => status,
    };

    let inspection = PackageInspection {
        entries: files
            .iter()
            .map(|(path, contents)| PackageEntry {
                path: path.clone(),
                byte_size: contents.len() as u64,
            })
            .collect(),
        signature,
    };
    let validated: ValidatedPluginPackage = validate_plugin_package(manifest, &inspection)
        .map_err(|errors| {
            format!("This package does not meet Orivo's plugin contract: {errors}")
        })?;

    // The policy check above proves the manifest is well formed; this proves
    // the bytes in the archive are the ones it describes.
    for artifact in &validated.manifest.manifest().artifacts {
        let contents = files
            .get(&artifact.path)
            .ok_or_else(|| "The package is missing a declared artifact.".to_string())?;
        if contents.len() as u64 != artifact.byte_size
            || !hex_digest(contents).eq_ignore_ascii_case(&artifact.sha256)
        {
            return Err("A package artifact does not match its manifest entry.".into());
        }
    }

    let runtime =
        PluginRuntime::shared().map_err(|_| "The plugin runtime is unavailable.".to_string())?;
    let component = files
        .get(COMPONENT_FILE)
        .ok_or_else(|| "The package has no component.".to_string())?;
    runtime
        .preflight_component(component)
        .map_err(|_| "The plugin component did not pass WebAssembly validation.".to_string())?;

    let plugin_id = validated.manifest.id().to_string();
    if !valid_plugin_id(&plugin_id) {
        return Err("The plugin identity is not usable as a directory.".into());
    }
    let version = validated.manifest.manifest().version.clone();
    // What the registry said it was sending, checked against what arrived.
    //
    // Only the id was compared before, and the id is not the part a downgrade
    // changes. "Is this an upgrade" is decided from the *index entry*, so an
    // entry announcing 2.0.0 that serves a 0.0.1 package — a stale asset, a
    // swapped release, a mirror that kept the old file under the new name —
    // walked the plugin backwards while every check passed.
    if let Some(expected) = expected {
        if expected.id != plugin_id || expected.version != version {
            return Err("The package does not contain the plugin the registry names.".into());
        }
    }
    let channel = match signature {
        PackageSignatureStatus::Trusted => PackageChannel::Official {
            signer: ORIVO_RELEASE_SIGNER.to_string(),
        },
        _ => PackageChannel::Development,
    };
    // `expected` is only ever `Some` through the registry doors
    // (`install_plugin_from_registry`, `update_plugin`, automatic maintenance):
    // that is exactly the channel with a downgrade rule to re-check under the
    // gate. A sideloaded package (`expected: None`) has no such rule.
    install_verified(
        service,
        &plugin_id,
        &version,
        channel,
        &files,
        expected.is_some(),
    )
    .map(|outcome| outcome.plugin_id)
}

/// The transaction, with the host's verdict wired into both of its checkpoints.
///
/// Separate from [`install_package`] because the channel is decided there by
/// the signature policy, and the junction tests need to drive a *signed*
/// install without holding Orivo's private key. Everything the transaction
/// actually does — staging, grading, the swap, the smoke test, the automatic
/// rollback — is this call, so those tests exercise the production path instead
/// of a re-creation of it.
pub(crate) fn install_verified(
    service: &PluginInstallerService,
    plugin_id: &str,
    version: &str,
    channel: PackageChannel,
    files: &PackageFiles,
    guard_against_downgrade: bool,
) -> Result<crate::plugin_update::InstallOutcome, String> {
    let runtime =
        PluginRuntime::shared().map_err(|_| "The plugin runtime is unavailable.".to_string())?;
    let registry = service.registry();
    let verify = |directory: &Path, checkpoint: Checkpoint| {
        // Staging cannot ask the component who it is: the directory is not
        // named after the plugin, and that name is half the question.
        // Everything else is cheaper to refuse there.
        let depth = match checkpoint {
            Checkpoint::Staged => VerifyDepth::Contract,
            Checkpoint::Live => VerifyDepth::Smoke,
        };
        verify_until_conclusive(|| registry.verify_package(&runtime, directory, plugin_id, depth))
    };
    if guard_against_downgrade {
        service
            .store
            .install_refusing_downgrade(plugin_id, version, channel, files, &verify)
    } else {
        service
            .store
            .install(plugin_id, version, channel, files, &verify)
    }
}

/// Ask the host for a verdict, and keep asking while it says it has none.
///
/// A refusal is final on the first answer. An *inconclusive* result is not an
/// answer at all — the probe was queued behind another job of the same plugin,
/// or the wall clock ran out on a loaded machine — and undoing a good update
/// because of it is the failure mode this exists to avoid. After the attempts
/// are spent the update is still rolled back, safely, but the message says the
/// host could not reach the plugin rather than that the plugin is broken.
fn verify_until_conclusive(ask: impl Fn() -> Result<(), PackageRefusal>) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..SMOKE_TEST_ATTEMPTS {
        match ask() {
            Ok(()) => return Ok(()),
            Err(PackageRefusal::Refused(message)) => return Err(message),
            Err(PackageRefusal::Inconclusive(message)) => {
                last = message;
                if attempt + 1 < SMOKE_TEST_ATTEMPTS {
                    std::thread::sleep(SMOKE_TEST_RETRY_DELAY);
                }
            }
        }
    }
    Err(format!(
        "Orivo could not check this plugin, so the version that was working was kept. ({last})"
    ))
}

/// Refuse a package the registry offers that is not newer than what is
/// installed.
///
/// Shared by both doors into the registry channel. They used to differ — only
/// `update_plugin` checked — and "install" was therefore the door a replayed
/// entry could walk a plugin backwards through.
fn refuse_a_downgrade(installed: Option<&str>, offered: &str) -> Result<(), String> {
    match installed {
        Some(installed) if !is_upgrade(offered, installed) => {
            Err("This plugin is already up to date.".into())
        }
        _ => Ok(()),
    }
}

/// Read the gzipped tar wholly in memory, bounded on entry count, per-entry
/// size and total size. Nothing is written to disk until the whole archive has
/// been read and accepted.
pub(crate) fn read_package(bytes: &[u8]) -> Result<PackageFiles, String> {
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err("This package is larger than Orivo allows.".into());
    }
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|_| "The package is not a readable archive.".to_string())?;
    let mut files = PackageFiles::new();
    let mut total = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|_| "The package archive is damaged.".to_string())?;
        if !entry.header().entry_type().is_file() {
            // Directories carry no payload, and a link could point anywhere on
            // the host. Only regular files are ever taken from a package.
            continue;
        }
        let path = entry
            .path()
            .map_err(|_| "The package contains an unreadable path.".to_string())?
            .to_string_lossy()
            .into_owned();
        if !safe_entry_path(&path) {
            return Err("The package contains an unsafe path.".into());
        }
        if files.len() >= MAX_PACKAGE_ENTRIES {
            return Err("The package contains too many files.".into());
        }
        let declared = entry.header().size().unwrap_or(0);
        total = total.saturating_add(declared);
        if declared > MAX_PACKAGE_BYTES || total > MAX_PACKAGE_BYTES {
            return Err("This package is larger than Orivo allows.".into());
        }
        let mut contents = Vec::with_capacity(declared.min(1024 * 1024) as usize);
        entry
            .take(MAX_PACKAGE_BYTES)
            .read_to_end(&mut contents)
            .map_err(|_| "The package archive is damaged.".to_string())?;
        if files.insert(path, contents).is_some() {
            return Err("The package declares the same file twice.".into());
        }
    }
    if files.is_empty() {
        return Err("The package is empty.".into());
    }
    Ok(files)
}

/// A package path is relative, has no traversal segment, and stays inside the
/// shallow shape the manifest policy allows.
fn safe_entry_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 256
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// The signature covers the SHA-256 digest of `manifest.json`, which in turn
/// pins every artifact by digest. Signing the manifest therefore signs the
/// whole package without the signer having to hash the archive framing.
fn signature_status(manifest_bytes: &[u8], signature: Option<&Vec<u8>>) -> PackageSignatureStatus {
    let Some(signature) = signature else {
        return PackageSignatureStatus::Missing;
    };
    let Ok(signature) = <[u8; 64]>::try_from(signature.as_slice()) else {
        return PackageSignatureStatus::Invalid;
    };
    let Some(key) = release_public_key() else {
        // No release key is compiled in, so provenance cannot be established
        // either way. This is not `Invalid` — that verdict is reserved for a
        // signature that demonstrably fails a key we hold, and reporting it
        // here would block the plugin author's own signed builds. Integrity is
        // unaffected: every artifact is checked against the manifest digests
        // regardless, and only the release channel requires `Trusted`.
        return PackageSignatureStatus::Development;
    };
    let digest = Sha256::digest(manifest_bytes);
    match key.verify_strict(&digest, &Signature::from_bytes(&signature)) {
        Ok(()) => PackageSignatureStatus::Trusted,
        Err(_) => PackageSignatureStatus::Invalid,
    }
}

fn release_public_key() -> Option<VerifyingKey> {
    let decoded = decode_base64(RELEASE_PUBLIC_KEY_BASE64)?;
    VerifyingKey::from_bytes(&<[u8; 32]>::try_from(decoded.as_slice()).ok()?).ok()
}

/// A tiny standard-alphabet decoder. The only base64 this crate reads is a
/// 32-byte key compiled into the binary, so a dependency would be more
/// surface than the four lines it replaces.
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    let mut accumulator = 0_u32;
    let mut bits = 0_u32;
    for byte in value.bytes() {
        if byte == b'=' {
            break;
        }
        let index = ALPHABET.iter().position(|candidate| *candidate == byte)? as u32;
        accumulator = (accumulator << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Some(output)
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_bounded_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, std::io::Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsafe file",
        ));
    }
    fs::read(path)
}

pub fn plugin_root_for(app_data: &Path) -> PathBuf {
    app_data.join(PLUGINS_DIRECTORY)
}

/// Whether *this component* is the one the install transaction accepted a
/// release signature for.
///
/// The runner host asks this the moment before it will invoke a package, with
/// the digest it has just re-derived from disk. It used to read
/// `.staging/trusted/<id>` for existence, which answers a weaker question — is
/// there a marker beside this plugin — and would still have said yes after
/// somebody dropped a different `component.wasm` into an installed one. The
/// answer here is about bytes, because that is what a grant is given to.
///
/// Free function rather than a method: the caller has a plugin root and not the
/// service, and every path it needs is derived from that root.
pub fn component_channel(
    plugin_root: &Path,
    plugin_id: &str,
    component_sha256: &str,
) -> PackageChannel {
    PluginStore::new(plugin_root.to_path_buf()).channel_for_component(plugin_id, component_sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
    /// The reference runner from `src-tauri/fixtures`. A runner package is only
    /// installable if its component really implements the contract, so a test
    /// about runners needs the real thing rather than an empty component.
    const RUNNER_COMPONENT: &[u8] = include_bytes!("../fixtures/orivo-runner-fixture.wasm");
    /// Must be the identity the fixture component reports.
    const RUNNER_ID: &str = "com.orivo.fixture-runner";
    const RUNNER_VERSION: &str = "1.0.0";

    /// A root no other test can land in. These run in parallel and write the
    /// same plugin directory names, so the clock alone is not enough.
    fn temporary_root(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-plugin-installer-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn manifest_json(id: &str, catalog: &[u8]) -> Vec<u8> {
        format!(
            r#"{{
              "id": "{id}",
              "name": "Quiky",
              "version": "0.1.0",
              "sdk": "orivo-plugin@1",
              "minOrivoVersion": "0.3.0",
              "extensions": ["installer"],
              "capabilities": ["network_fetch"],
              "networkDomains": ["cdn.openttd.org"],
              "artifacts": [
                {{"path":"component.wasm","kind":"component","sha256":"{}","byteSize":{}}},
                {{"path":"assets/catalog.json","kind":"asset","sha256":"{}","byteSize":{}}}
              ]
            }}"#,
            hex_digest(EMPTY_COMPONENT),
            EMPTY_COMPONENT.len(),
            hex_digest(catalog),
            catalog.len(),
        )
        .into_bytes()
    }

    /// A runner package, whose manifest version the caller chooses. The fixture
    /// component always reports `1.0.0`, so any other version is a package
    /// whose component was not rebuilt — the realistic way an update fails its
    /// smoke test.
    fn runner_manifest_json(version: &str, capabilities: &str) -> Vec<u8> {
        format!(
            r#"{{
              "id": "{RUNNER_ID}",
              "name": "Fixture Runner",
              "version": "{version}",
              "sdk": "orivo-plugin@1",
              "minOrivoVersion": "0.3.0",
              "extensions": ["runner"],
              "capabilities": [{capabilities}],
              "networkDomains": [],
              "artifacts": [
                {{"path":"component.wasm","kind":"component","sha256":"{}","byteSize":{}}}
              ]
            }}"#,
            hex_digest(RUNNER_COMPONENT),
            RUNNER_COMPONENT.len(),
        )
        .into_bytes()
    }

    fn package(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_slice())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn valid_package(id: &str) -> Vec<u8> {
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        package(&[
            ("manifest.json", manifest_json(id, &catalog)),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            ("assets/catalog.json", catalog),
        ])
    }

    fn runner_package(version: &str) -> Vec<u8> {
        package(&[
            (
                "manifest.json",
                runner_manifest_json(version, r#""runner_prepare","files_read""#),
            ),
            ("component.wasm", RUNNER_COMPONENT.to_vec()),
        ])
    }

    fn service(root: &Path) -> PluginInstallerService {
        PluginInstallerService::new(root.to_path_buf(), "0.3.0")
    }

    #[test]
    fn an_unsigned_package_installs_only_through_the_sideload_channel() {
        let root = temporary_root("sideload");
        let service = service(&root);
        let bytes = valid_package("com.orivo.quiky");

        assert_eq!(
            install_package(&service, &bytes, SignaturePolicy::ReleaseOnly, None),
            Err("This package is not signed by Orivo.".into())
        );
        assert!(!root.join("com.orivo.quiky").exists());

        let installed = install_package(&service, &bytes, SignaturePolicy::AllowUnsigned, None)
            .expect("installs");
        assert_eq!(installed, "com.orivo.quiky");
        assert!(root.join("com.orivo.quiky/manifest.json").is_file());
        assert!(root.join("com.orivo.quiky/component.wasm").is_file());
        assert!(root.join("com.orivo.quiky/assets/catalog.json").is_file());
        // A sideloaded package is never marked trusted.
        assert!(!service.store.is_trusted("com.orivo.quiky"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_package_whose_artifact_bytes_changed_is_refused() {
        let root = temporary_root("tampered");
        let service = service(&root);
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        let bytes = package(&[
            ("manifest.json", manifest_json("com.orivo.quiky", &catalog)),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            // The manifest still pins the original digest and length.
            (
                "assets/catalog.json",
                br#"{"version":1,"titles":[ ]}"#.to_vec(),
            ),
        ]);

        assert_eq!(
            install_package(&service, &bytes, SignaturePolicy::AllowUnsigned, None),
            Err("A package artifact does not match its manifest entry.".into())
        );
        assert!(!root.join("com.orivo.quiky").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_package_carrying_an_undeclared_payload_is_refused() {
        let root = temporary_root("payload");
        let service = service(&root);
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        let bytes = package(&[
            ("manifest.json", manifest_json("com.orivo.quiky", &catalog)),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            ("assets/catalog.json", catalog),
            ("aria2c.exe", b"MZ".to_vec()),
        ]);

        assert!(
            install_package(&service, &bytes, SignaturePolicy::AllowUnsigned, None)
                .is_err_and(|error| error.contains("plugin contract"))
        );
        assert!(!root.join("com.orivo.quiky").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_traversal_path_never_leaves_the_plugin_root() {
        assert!(safe_entry_path("assets/catalog.json"));
        assert!(!safe_entry_path("../escape.json"));
        assert!(!safe_entry_path("/etc/passwd"));
        assert!(!safe_entry_path("assets/../../escape.json"));
        assert!(!safe_entry_path("assets\\catalog.json"));
    }

    #[test]
    fn signature_verdicts_separate_absence_from_malformation() {
        let manifest = b"{}";
        assert_eq!(
            signature_status(manifest, None),
            PackageSignatureStatus::Missing
        );
        // A signature of the wrong length can never be evaluated by anyone.
        assert_eq!(
            signature_status(manifest, Some(&vec![0_u8; 8])),
            PackageSignatureStatus::Invalid
        );
        // Well formed, but it does not verify against the compiled release
        // key, and that is exactly what `Invalid` is reserved for.
        assert_eq!(
            signature_status(manifest, Some(&vec![0_u8; 64])),
            PackageSignatureStatus::Invalid
        );
    }

    /// The end-to-end check against the real artefact the plugin project
    /// builds: its signature verifies against the compiled release key, so it
    /// is accepted by the strict registry channel and marked trusted on disk.
    #[test]
    fn the_plugin_projects_own_package_is_trusted_by_the_release_key() {
        let artefact = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace")
            .join("plugin-quiky/dist/com.orivo.quiky-0.1.0.orivo-plugin");
        let Ok(bytes) = fs::read(&artefact) else {
            // The plugin is a separate project; skip when it is not built.
            return;
        };
        let root = temporary_root("real-package");
        let service = service(&root);

        let id = install_package(&service, &bytes, SignaturePolicy::ReleaseOnly, None)
            .expect("the released package installs through the strict channel");
        assert_eq!(id, "com.orivo.quiky");
        assert!(root.join("com.orivo.quiky/manifest.json").is_file());
        assert!(root.join("com.orivo.quiky/assets/catalog.json").is_file());
        // The signature file itself is never written into the plugin tree.
        assert!(!root.join("com.orivo.quiky/signature.ed25519").exists());
        assert!(
            service.store.is_trusted("com.orivo.quiky"),
            "marked trusted"
        );

        // A manifest edited after signing breaks the signature, and the strict
        // channel is the one that must notice.
        let files = read_package(&bytes).unwrap();
        let mut tampered = files.clone();
        let mut manifest = tampered.get("manifest.json").unwrap().clone();
        manifest.push(b' ');
        tampered.insert("manifest.json".into(), manifest);
        let repacked = package(
            &tampered
                .iter()
                .map(|(path, contents)| (path.as_str(), contents.clone()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            install_package(&service, &repacked, SignaturePolicy::ReleaseOnly, None),
            Err("This package is not signed by Orivo.".into())
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn base64_decodes_a_key_and_rejects_rubbish() {
        assert_eq!(decode_base64("QUJD").as_deref(), Some(&b"ABC"[..]));
        assert_eq!(decode_base64(""), None);
        assert_eq!(decode_base64("!!!!"), None);
    }

    /// P1's open follow-up, closed: the installer used to accept any package
    /// whose component merely compiled, and discovery then showed it as a
    /// broken row.
    ///
    /// Two refusals, and what matters is *where* each lands. Both are contract
    /// failures, so both are caught in staging — before the running plugin is
    /// displaced and before the rollback slot is spent. Asserting the slot
    /// survives is the only way to tell that from a refusal that swapped, was
    /// rejected at the final path, and rolled itself back: the user-visible
    /// error is identical either way.
    #[test]
    fn a_runner_package_that_fails_the_contract_is_refused_before_it_displaces_anything() {
        let broken_component = String::from_utf8(runner_manifest_json(
            RUNNER_VERSION,
            r#""runner_prepare","files_read""#,
        ))
        .unwrap()
        .replace(&hex_digest(RUNNER_COMPONENT), &hex_digest(EMPTY_COMPONENT))
        .replace(
            &format!(r#""byteSize":{}"#, RUNNER_COMPONENT.len()),
            &format!(r#""byteSize":{}"#, EMPTY_COMPONENT.len()),
        );
        let cases: [(&str, Vec<u8>, &str); 2] = [
            (
                "not-a-runner",
                package(&[
                    ("manifest.json", broken_component.into_bytes()),
                    ("component.wasm", EMPTY_COMPONENT.to_vec()),
                ]),
                "This plugin does not implement Orivo's runner contract.",
            ),
            (
                // A component needing a capability its manifest never listed
                // was agreed to under a false description.
                "undeclared",
                package(&[
                    (
                        "manifest.json",
                        runner_manifest_json(RUNNER_VERSION, r#""runner_prepare""#),
                    ),
                    ("component.wasm", RUNNER_COMPONENT.to_vec()),
                ]),
                "This plugin needs more permissions than its manifest declares.",
            ),
        ];

        for (label, bytes, refusal) in cases {
            let root = temporary_root(label);
            let service = service(&root);
            // Nothing installed yet: the package is refused outright.
            assert_eq!(
                install_package(&service, &bytes, SignaturePolicy::AllowUnsigned, None),
                Err(refusal.into()),
                "{label}"
            );
            assert!(!root.join(RUNNER_ID).exists(), "{label}");

            // And with a working plugin in place, it stays in place — with the
            // way back to its own predecessor intact.
            for _ in 0..2 {
                install_package(
                    &service,
                    &runner_package(RUNNER_VERSION),
                    SignaturePolicy::AllowUnsigned,
                    None,
                )
                .expect("the fixture runner installs");
            }
            assert_eq!(
                install_package(&service, &bytes, SignaturePolicy::AllowUnsigned, None),
                Err(refusal.into()),
                "{label}"
            );
            assert_eq!(
                installed_version(&service, RUNNER_ID).as_deref(),
                Some(RUNNER_VERSION),
                "{label}"
            );
            assert_eq!(
                service
                    .store
                    .rollback_target(RUNNER_ID)
                    .map(|target| target.version),
                Some(RUNNER_VERSION.into()),
                "{label}: the transaction started for a package it could refuse in staging"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    /// The whole point of the transaction, through the real host: an update
    /// whose component no longer matches its manifest fails the smoke test at
    /// its final path, and the version that worked comes back.
    #[test]
    fn an_update_that_fails_the_hosts_smoke_test_restores_the_previous_version() {
        let root = temporary_root("smoke-rollback");
        let service = service(&root);
        install_package(
            &service,
            &runner_package(RUNNER_VERSION),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .expect("the fixture runner installs");
        assert_eq!(
            installed_version(&service, RUNNER_ID).as_deref(),
            Some(RUNNER_VERSION)
        );

        // A release whose manifest was bumped and whose component was not. It
        // passes every package check and the contract check, and only the
        // component's own account of itself gives it away.
        let refusal = install_package(
            &service,
            &runner_package("2.0.0"),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .expect_err("the smoke test refuses it");
        assert!(
            refusal.contains("does not match the package"),
            "unexpected refusal: {refusal}"
        );

        assert_eq!(
            installed_version(&service, RUNNER_ID).as_deref(),
            Some(RUNNER_VERSION),
            "the working version was not restored"
        );
        // And discovery agrees: the plugin is usable, not a broken row.
        let runtime = PluginRuntime::shared().unwrap();
        let plugins = service.registry().runner_plugins(&runtime);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].state, PluginState::Ready);
        assert_eq!(plugins[0].version, RUNNER_VERSION);
        fs::remove_dir_all(&root).ok();
    }

    /// The registry entry is what "is this an upgrade" is decided from, and
    /// nothing used to bind the package that arrived to what the entry said. A
    /// stale asset, a swapped release or a mirror still serving the old file
    /// under the new name walked the plugin backwards with every check passing.
    #[test]
    fn a_package_that_is_not_what_the_registry_named_is_refused() {
        let root = temporary_root("promise");
        let service = service(&root);
        let bytes = valid_package("com.orivo.quiky");

        // The right id, an older version than the entry announced.
        assert_eq!(
            install_package(
                &service,
                &bytes,
                SignaturePolicy::AllowUnsigned,
                Some(&PackagePromise {
                    id: "com.orivo.quiky".into(),
                    version: "0.9.0".into(),
                }),
            ),
            Err("The package does not contain the plugin the registry names.".into())
        );
        // The right version, another plugin entirely.
        assert_eq!(
            install_package(
                &service,
                &bytes,
                SignaturePolicy::AllowUnsigned,
                Some(&PackagePromise {
                    id: "com.orivo.other".into(),
                    version: "0.1.0".into(),
                }),
            ),
            Err("The package does not contain the plugin the registry names.".into())
        );
        assert!(!root.join("com.orivo.quiky").exists());

        // What the entry actually named installs.
        assert_eq!(
            install_package(
                &service,
                &bytes,
                SignaturePolicy::AllowUnsigned,
                Some(&PackagePromise {
                    id: "com.orivo.quiky".into(),
                    version: "0.1.0".into(),
                }),
            ),
            Ok("com.orivo.quiky".into())
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Installing and updating are the same transaction, so they need the same
    /// guard. Only `update_plugin` had one, which made "install" the door a
    /// replayed registry entry could walk an installed plugin backwards through.
    #[test]
    fn neither_door_into_the_registry_channel_accepts_a_downgrade() {
        assert!(refuse_a_downgrade(None, "0.1.0").is_ok());
        assert!(refuse_a_downgrade(Some("0.1.0"), "0.2.0").is_ok());
        assert_eq!(
            refuse_a_downgrade(Some("0.2.0"), "0.1.0"),
            Err("This plugin is already up to date.".into())
        );
        assert_eq!(
            refuse_a_downgrade(Some("0.2.0"), "0.2.0"),
            Err("This plugin is already up to date.".into())
        );
        // Unorderable on either side is not an upgrade either.
        assert_eq!(
            refuse_a_downgrade(Some("0.2.0"), "latest"),
            Err("This plugin is already up to date.".into())
        );
    }

    /// A smoke test runs through the scheduler, which allows one job at a time
    /// per plugin. A discovery page already in flight can push the probe past
    /// its bounded wait, and undoing a good update for that would make an
    /// update's success depend on how busy the machine is.
    #[test]
    fn an_inconclusive_smoke_test_is_asked_again_before_anything_is_undone() {
        use std::cell::Cell;

        // Contention that clears: the first answer is no answer, the second is.
        let attempts = Cell::new(0);
        assert_eq!(
            verify_until_conclusive(|| {
                attempts.set(attempts.get() + 1);
                if attempts.get() < 2 {
                    Err(PackageRefusal::Inconclusive("busy".into()))
                } else {
                    Ok(())
                }
            }),
            Ok(())
        );
        assert_eq!(attempts.get(), 2);

        // A verdict is final on the first answer; retrying a real refusal would
        // only make a broken package slow to refuse.
        let attempts = Cell::new(0);
        assert_eq!(
            verify_until_conclusive(|| {
                attempts.set(attempts.get() + 1);
                Err(PackageRefusal::Refused("this is not a runner".into()))
            }),
            Err("this is not a runner".into())
        );
        assert_eq!(attempts.get(), 1);

        // Contention that does not clear still rolls back — safely — but says
        // which kind of failure it was rather than blaming the package.
        let attempts = Cell::new(0);
        let exhausted = verify_until_conclusive(|| {
            attempts.set(attempts.get() + 1);
            Err(PackageRefusal::Inconclusive("busy".into()))
        })
        .expect_err("gives up in the end");
        assert_eq!(attempts.get(), SMOKE_TEST_ATTEMPTS);
        assert!(exhausted.contains("could not check"), "{exhausted}");
    }

    /// The interface E2 wires itself to after this merges: for the live version
    /// of a plugin, what package it actually is. A grant is an agreement with
    /// this, not with the id.
    #[test]
    fn the_package_identity_names_the_component_that_will_run() {
        let root = temporary_root("identity");
        let service = service(&root);
        install_package(
            &service,
            &runner_package(RUNNER_VERSION),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .unwrap();

        let identity = service
            .package_identity(RUNNER_ID)
            .expect("an installed plugin has an identity");
        assert_eq!(identity.plugin_id, RUNNER_ID);
        assert_eq!(identity.version, RUNNER_VERSION);
        assert_eq!(identity.component_sha256, hex_digest(RUNNER_COMPONENT));
        assert_eq!(identity.channel, PackageChannel::Development);

        service.uninstall(RUNNER_ID).unwrap();
        assert_eq!(service.package_identity(RUNNER_ID), None);
        fs::remove_dir_all(&root).ok();
    }

    /// The plan's exit test for this step, at the level this module owns: a
    /// plugin that goes forward and back leaves the version the user can return
    /// to, and the catalogue keeps telling the truth about both.
    #[test]
    fn an_update_and_a_rollback_are_both_visible_in_the_catalogue() {
        let root = temporary_root("rollback-view");
        let service = service(&root);
        install_package(
            &service,
            &valid_package("com.orivo.quiky"),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .unwrap();
        let installed = service.installed();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].rollback_to, None);
        assert_eq!(installed[0].rollback_trusted, None);

        // The same identity at a later version: an ordinary update.
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        let manifest = String::from_utf8(manifest_json("com.orivo.quiky", &catalog))
            .unwrap()
            .replace(r#""version": "0.1.0""#, r#""version": "0.2.0""#);
        let newer = package(&[
            ("manifest.json", manifest.into_bytes()),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            ("assets/catalog.json", catalog),
        ]);
        install_package(&service, &newer, SignaturePolicy::AllowUnsigned, None).unwrap();

        let installed = service.installed();
        assert_eq!(installed[0].version, "0.2.0");
        assert_eq!(installed[0].rollback_to.as_deref(), Some("0.1.0"));
        // Both versions arrived through the unsigned door in this test, so the
        // version to go back to is unsigned too — Settings must not offer it
        // as if it were as safe as an official one.
        assert_eq!(installed[0].rollback_trusted, Some(false));

        assert_eq!(
            service.store.rollback(&installed[0].id).unwrap().version,
            "0.1.0"
        );
        let installed = service.installed();
        assert_eq!(installed[0].version, "0.1.0");
        assert_eq!(installed[0].rollback_to, None);
        assert_eq!(installed[0].rollback_trusted, None);
        fs::remove_dir_all(&root).ok();
    }

    /// The counterpart of the test above: when both the live version and the
    /// one it can go back to arrived through the registry, Settings has to be
    /// able to say the rollback target is signed rather than defaulting to the
    /// unsigned warning the sideload path exercises.
    #[test]
    fn a_rollback_target_installed_through_the_registry_is_reported_trusted() {
        let root = temporary_root("rollback-trust");
        let service = service(&root);
        let official = || crate::plugin_update::PackageChannel::Official {
            signer: "test-release-key".into(),
        };

        let first = read_package(&valid_package("com.orivo.quiky")).unwrap();
        install_verified(
            &service,
            "com.orivo.quiky",
            "0.1.0",
            official(),
            &first,
            false,
        )
        .unwrap();

        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        let manifest = String::from_utf8(manifest_json("com.orivo.quiky", &catalog))
            .unwrap()
            .replace(r#""version": "0.1.0""#, r#""version": "0.2.0""#);
        let newer_bytes = package(&[
            ("manifest.json", manifest.into_bytes()),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            ("assets/catalog.json", catalog),
        ]);
        let newer = read_package(&newer_bytes).unwrap();
        install_verified(
            &service,
            "com.orivo.quiky",
            "0.2.0",
            official(),
            &newer,
            true,
        )
        .unwrap();

        let installed = service.installed();
        assert_eq!(installed[0].rollback_to.as_deref(), Some("0.1.0"));
        assert_eq!(installed[0].rollback_trusted, Some(true));
        fs::remove_dir_all(&root).ok();
    }

    /// The plan's exit test for step 3.1, in full: *a plugin rollback keeps the
    /// imported games and the compatible profiles.*
    ///
    /// It is a structural property — the library, the runner profiles and the
    /// preferences are Orivo's own files, and the plugin transaction only ever
    /// renames directories inside the plugin root — but structural properties
    /// are exactly the ones that quietly stop holding. So this exercises the
    /// real `Catalog` and the real `PreferencesService` over the same app-data
    /// directory the plugin root lives in, and compares the whole record.
    #[test]
    fn an_update_and_a_rollback_keep_the_imported_games_and_the_profiles() {
        use crate::catalog::{Catalog, WineGraphicsOptions, WineProfile};
        use crate::preferences::{PreferencesService, PreferencesUpdate};

        let app_data = temporary_root("user-data");
        let plugin_root = plugin_root_for(&app_data);
        fs::create_dir_all(&plugin_root).unwrap();
        let service = PluginInstallerService::new(plugin_root, "0.3.0");

        let catalog_path = app_data.join("catalog.json");
        let mut catalog = Catalog::default();
        catalog
            .add(
                crate::catalog::Game::from_executable(
                    "/Games/Nightfall/Nightfall.app/Contents/MacOS/Nightfall",
                )
                .unwrap(),
            )
            .unwrap();
        assert!(
            catalog
                .upsert_wine_profile(WineProfile {
                    id: "wine-profile-1".into(),
                    display_name: "Windows classics".into(),
                    wine_binary: PathBuf::from("/Applications/Wine.app/Contents/bin/wine"),
                    prefix: app_data.join("wine-prefixes/wine-profile-1"),
                    game_directories: vec![PathBuf::from("/Games/Windows")],
                    graphics: WineGraphicsOptions::default(),
                    dxmt_engine_supported: None,
                    macos_retina_mode_enabled: None,
                    enabled: true,
                    last_imported_at: Some(1_721_553_600_000),
                })
                .unwrap()
        );
        catalog.save_atomically(&catalog_path).unwrap();

        let preferences = PreferencesService::new(app_data.clone(), app_data.join("cache"));
        let chosen = preferences
            .update(PreferencesUpdate {
                beta_features: Some(true),
                ..Default::default()
            })
            .unwrap();

        install_package(
            &service,
            &runner_package(RUNNER_VERSION),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .expect("the fixture runner installs");
        // An update that works, the rollback a user asks for, and the one the
        // host performs itself when a component fails its smoke test.
        install_package(
            &service,
            &runner_package(RUNNER_VERSION),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .expect("reinstalling is an update like any other");
        assert_eq!(
            service.store.rollback(RUNNER_ID).unwrap().version,
            RUNNER_VERSION
        );
        install_package(
            &service,
            &runner_package("2.0.0"),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .expect_err("the smoke test refuses a component that was not rebuilt");
        assert_eq!(
            installed_version(&service, RUNNER_ID).as_deref(),
            Some(RUNNER_VERSION)
        );

        assert_eq!(
            Catalog::load(&catalog_path).unwrap(),
            catalog,
            "the library or a runner profile changed under a plugin transaction"
        );
        assert_eq!(preferences.load().unwrap(), chosen);
        fs::remove_dir_all(&app_data).ok();
    }

    /// Automatic updates are opt-in and cover the official channel only. A
    /// sideloaded package has no trust marker, so consent can never be what
    /// lets an unsigned build replace itself behind the user's back.
    #[test]
    fn consent_is_required_and_never_reaches_the_developer_channel() {
        let root = temporary_root("policy");
        let service = service(&root);
        assert!(!service.policy().automatic);
        service
            .set_policy(&PluginUpdatePolicy { automatic: true })
            .unwrap();
        assert!(service.policy().automatic);

        install_package(
            &service,
            &valid_package("com.orivo.quiky"),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .unwrap();
        // The compiled-in registry lists a newer Quiky than the fixture
        // package, and this install is a development build all the same.
        assert!(!service.store.is_trusted("com.orivo.quiky"));
        assert_eq!(pending_automatic_updates(&service), Vec::new());
        fs::remove_dir_all(&root).ok();
    }

    /// What the startup update check reads, and all it reads.
    ///
    /// `pending_automatic_updates` runs at launch for a user who consented to
    /// updates, and it used to get there through `installed()` — which preflights
    /// every installed component, so launching Orivo compiled every plugin, on the
    /// command executor, and (once a compile cache existed behind
    /// `prepare_component`) read that cache's install key at the same time. The
    /// three fields it actually wants are the manifest's and the trust marker's.
    ///
    /// `valid_package` ships `EMPTY_COMPONENT`, which is a core module and not a
    /// component: Wasmtime refuses it. That it is listed here with its id and
    /// version anyway is the property — no component was consulted to produce
    /// this answer.
    #[test]
    fn the_update_check_reads_manifests_and_not_components() {
        let root = temporary_root("update-listing");
        let service = service(&root);
        install_package(
            &service,
            &valid_package("com.orivo.quiky"),
            SignaturePolicy::AllowUnsigned,
            None,
        )
        .unwrap();

        let listed = service.registry().installed_manifests();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "com.orivo.quiky");
        assert!(!listed[0].version.is_empty());

        // And the whole call, at the call site, compiling nothing. Counted per
        // thread, so the parallel suite cannot move it: putting `installed()`
        // back here fails this line and nothing else would.
        let before = PluginRuntime::compiles_on_this_thread();
        // The consent rule still holds on top of it: a sideloaded package has no
        // trust marker, so nothing is pending.
        assert_eq!(pending_automatic_updates(&service), Vec::new());
        assert_eq!(
            PluginRuntime::compiles_on_this_thread(),
            before,
            "the startup update check compiled a component"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// The display path must never reach the network, so the catalogue is built
    /// from the compiled-in list and the cache alone. The compiled-in list is
    /// held to the same grammar as a downloaded one.
    #[test]
    fn the_catalogue_is_served_without_a_network_call() {
        let root = temporary_root("catalogue");
        let service = service(&root);
        let catalog = service.catalog();
        assert!(catalog.installed.is_empty());
        assert!(
            catalog
                .available
                .iter()
                .any(|entry| entry.id == "com.orivo.quiky"),
            "the compiled-in registry survived validation"
        );
        assert!(
            catalog
                .available
                .iter()
                .all(|entry| entry.size_bytes > 0 && !entry.id.is_empty())
        );
        fs::remove_dir_all(&root).ok();
    }

    /// An artefact of the plugin project, not of this crate: if the compiled-in
    /// registry stops parsing, the Store silently loses its only entry. The
    /// grammar that rejects a downloaded index rejects this one too.
    #[test]
    fn the_compiled_in_registry_passes_the_same_grammar_as_a_downloaded_one() {
        let entries = crate::plugin_index::parse_unsigned_entries(REGISTRY_JSON.as_bytes());
        assert_eq!(
            entries.len(),
            serde_json::from_str::<Vec<serde_json::Value>>(REGISTRY_JSON)
                .unwrap()
                .len(),
            "an entry of resources/plugin-registry.json was dropped by validation"
        );
    }
}
