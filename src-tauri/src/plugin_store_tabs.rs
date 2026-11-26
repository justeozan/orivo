//! The Store tabs plugins add, as the Store asks for them.
//!
//! A plugin declares the `store_tab` extension and exports `store-tabs`; this
//! module finds those packages, calls them through the same runtime, scheduler,
//! fuel and deadline as every other plugin call, and hands the WebView what the
//! runtime validated — never what the plugin sent. `plugin_runtime.rs` owns the
//! validation; this file owns *when* a plugin is asked and what is remembered.
//!
//! Three decisions shape it.
//!
//! * **One answer per package, not per visit.** The `store-tab-plugin` world
//!   imports nothing that could vary between calls — no clock, no network, no
//!   file — so a component's tabs are a function of its bytes. The answer is
//!   kept against the component's digest and asked again only when that digest
//!   changes. A Store visit after the first therefore costs reading the plugin
//!   folder — manifests and digests — and no compilation, and a refusal the
//!   package will repeat is remembered too.
//! * **Showing a page asks the keychain nothing.** Nothing here opens the
//!   compile cache: showing a page is not something the user did *about a
//!   plugin*, which is what `plugin_compile_cache::permit` waits for. A package
//!   is compiled without the cache instead, once per digest per session.
//! * **Gone means gone.** A package that is no longer installed, no longer
//!   valid, or that the scheduler parked after repeated failures has no tab, and
//!   the game pages it offered stop opening with it. `lib.rs` also tells the
//!   WebView when a package changes, so the bar follows without a visit.
//!
//! The page a game links to is the one value that does not cross: the WebView
//! is told that a game has one, and asks to open it by the game's id.

use crate::plugin_manifest::{HostCompatibility, valid_opaque_id};
use crate::plugin_registry::{PackageRefusal, PluginRegistry, StoreTabPackage};
use crate::plugin_runtime::{
    PluginGrants, PluginInvocation, PluginRequest, PluginResponse, PluginRuntime,
    PluginRuntimeError, PluginStoreTab, next_correlation_id,
};
use crate::plugin_scheduler::{JobError, JobHandle, SubmitError};
use crate::store::UrlOpener;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Sent to the main window when an installed package changes or goes away, so
/// the Store's bar can ask again without waiting for the next visit.
pub const PLUGIN_STORE_TABS_EVENT: &str = "plugin-store-tabs-changed";

/// How much longer than one call's own deadline a listing waits for all of its
/// calls together. The extra is queueing behind other plugin work; the
/// components themselves are already stopped by their epoch deadline.
const LISTING_WAIT_MULTIPLIER: u32 = 4;
/// `<plugin id>:<game id>`: the manifest's bound, a colon, the runtime's.
const MAX_GAME_ID_BYTES: usize = 128 + 1 + 64;

// ---------------------------------------------------------------------------
// IPC views
// ---------------------------------------------------------------------------

/// One tab, as the WebView receives it. Mirrors `src/plugin-store-tabs.ts`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreTabView {
    pub plugin_id: String,
    pub key: Option<String>,
    pub label: String,
    pub accent: Option<String>,
    pub games: Vec<StoreTabGameView>,
}

/// Empty strings for missing pictures, as `GameSummary` spells them.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreTabGameView {
    pub id: String,
    pub title: String,
    pub short_description: String,
    pub cover_url: String,
    pub hero_url: String,
    pub landscape_url: String,
    pub logo_url: String,
    pub genres: Vec<String>,
    pub price: Option<StoreTabPriceView>,
    /// Whether `open_plugin_store_page` has somewhere to go for this game.
    pub has_page: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreTabPriceView {
    pub currency: String,
    pub amount_minor: u32,
    pub regular_amount_minor: Option<u32>,
}

fn view(plugin_id: &str, tab: &PluginStoreTab) -> StoreTabView {
    StoreTabView {
        plugin_id: plugin_id.to_owned(),
        key: tab.key.clone(),
        label: tab.label.clone(),
        accent: tab.accent.clone(),
        games: tab
            .games
            .iter()
            .map(|game| StoreTabGameView {
                id: game.id.clone(),
                title: game.title.clone(),
                short_description: game.short_description.clone().unwrap_or_default(),
                cover_url: game.cover_url.clone().unwrap_or_default(),
                hero_url: game.hero_url.clone().unwrap_or_default(),
                landscape_url: game.landscape_url.clone().unwrap_or_default(),
                logo_url: game.logo_url.clone().unwrap_or_default(),
                genres: game.genres.clone(),
                price: game.price.as_ref().map(|price| StoreTabPriceView {
                    currency: price.currency.clone(),
                    amount_minor: price.amount_minor,
                    regular_amount_minor: price.regular_amount_minor,
                }),
                has_page: game.page_url.is_some(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

/// What one package said, for the digest it said it under. A refusal the
/// package would repeat is kept as no tabs, so it is not recompiled per visit.
#[derive(Debug, Clone)]
struct Answer {
    sha256: String,
    tabs: Arc<Vec<PluginStoreTab>>,
}

pub struct PluginStoreTabs {
    registry: PluginRegistry,
    /// `false` where this build may not run a downloaded plugin at all — iOS
    /// and the Mac App Store edition (`platform.rs`). Such a build lists no tab
    /// and opens no page, whatever is on disk.
    enabled: bool,
    answers: Mutex<BTreeMap<String, Answer>>,
}

impl std::fmt::Debug for PluginStoreTabs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginStoreTabs")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl PluginStoreTabs {
    pub fn new(plugin_root: PathBuf, host_version: &'static str, enabled: bool) -> Self {
        Self {
            registry: PluginRegistry::new(plugin_root, HostCompatibility::v1(host_version)),
            enabled,
            answers: Mutex::new(BTreeMap::new()),
        }
    }

    fn answers(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Answer>> {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Every tab of every installed package that declares one, in discovery
    /// order. `runtime` is only called when there is a package to ask, so a
    /// machine without such a plugin never starts the plugin engine for this.
    pub fn tabs(
        &self,
        runtime: impl FnOnce() -> Result<PluginRuntime, PluginRuntimeError>,
    ) -> Vec<StoreTabView> {
        if !self.enabled {
            return Vec::new();
        }
        let packages = self.registry.store_tab_packages();
        {
            // A package that left, or became other code, takes its answer — and
            // the pages that answer could open — with it.
            let mut answers = self.answers();
            answers.retain(|id, answer| {
                packages
                    .iter()
                    .any(|package| package.id() == id && package.sha256() == answer.sha256)
            });
        }
        if packages.is_empty() {
            return Vec::new();
        }
        let Ok(runtime) = runtime() else {
            return Vec::new();
        };
        // Parked after repeated failures: no tab, and no page, until someone
        // resumes it — and then it is asked again, like a package never seen.
        let (live, parked): (Vec<StoreTabPackage>, Vec<StoreTabPackage>) = packages
            .into_iter()
            .partition(|package| !runtime.scheduler().health(package.id()).degraded);
        let unanswered: Vec<&StoreTabPackage> = {
            let mut answers = self.answers();
            for package in &parked {
                answers.remove(package.id());
            }
            live.iter()
                .filter(|package| !answers.contains_key(package.id()))
                .collect()
        };
        for (plugin_id, answer) in ask(&runtime, &unanswered) {
            self.answers().insert(plugin_id, answer);
        }

        let answers = self.answers();
        live.iter()
            .filter_map(|package| {
                let answer = answers.get(package.id())?;
                Some(answer.tabs.iter().map(|tab| view(package.id(), tab)))
            })
            .flatten()
            .collect()
    }

    /// The page a listed game links to, while its package is still the one
    /// that listed it. `None` for an id nobody listed, which is all a WebView
    /// that names some other game learns.
    pub fn page_url(&self, game_id: &str) -> Option<String> {
        if !self.enabled || !valid_opaque_id(game_id, MAX_GAME_ID_BYTES) {
            return None;
        }
        let (plugin_id, _) = game_id.split_once(':')?;
        let answers = self.answers();
        answers
            .get(plugin_id)?
            .tabs
            .iter()
            .flat_map(|tab| tab.games.iter())
            .find(|game| game.id == game_id)?
            .page_url
            .clone()
    }

    /// Open that page with `opener`. The address was checked when the plugin
    /// answered; it is handed over exactly as the runtime normalised it.
    pub fn open_page(&self, game_id: &str, opener: &dyn UrlOpener) -> Result<(), String> {
        let url = self
            .page_url(game_id)
            .ok_or_else(|| "This page is no longer available.".to_string())?;
        opener.open(&url)
    }

    /// Drop what a package said. Called when it becomes another package or is
    /// removed, so a page it listed stops opening at once rather than at the
    /// next listing.
    pub fn forget(&self, plugin_id: &str) {
        self.answers().remove(plugin_id);
    }
}

/// Ask each package once, all of them queued before any is waited on, under
/// one wall-clock bound for the whole listing.
fn ask(runtime: &PluginRuntime, packages: &[&StoreTabPackage]) -> Vec<(String, Answer)> {
    let mut pending: Vec<(&StoreTabPackage, JobHandle<PluginInvocation>)> = Vec::new();
    let mut answered = Vec::new();
    for package in packages {
        let prepared = match package.prepare(runtime) {
            Ok(prepared) => prepared,
            Err(reason) => {
                // A package that does not keep the contract will not start to on
                // the next visit; remember it answered nothing.
                runtime.journal().record(
                    next_correlation_id(),
                    package.id(),
                    "store-tabs-refused",
                    reason,
                );
                answered.push((package.id().to_owned(), refused(package)));
                continue;
            }
        };
        let grants = PluginGrants::declared_only(package.manifest());
        match runtime.submit(&prepared, package.id(), &grants, PluginRequest::StoreTabs) {
            Ok(handle) => pending.push((package, handle)),
            // Busy or parked: no answer this time, and nothing to remember.
            Err(
                SubmitError::Busy { .. } | SubmitError::Degraded { .. } | SubmitError::ShuttingDown,
            ) => {}
        }
    }

    let deadline = Instant::now()
        + runtime
            .limits()
            .interactive_deadline
            .saturating_mul(LISTING_WAIT_MULTIPLIER);
    for (package, handle) in pending {
        let budget = deadline.saturating_duration_since(Instant::now());
        let outcome = match handle.wait_for(budget.max(Duration::from_millis(1))) {
            Ok(Ok(invocation)) => match invocation.response {
                PluginResponse::StoreTabs(tabs) => Ok(tabs),
                _ => Err(PluginRuntimeError::InvalidResult("store tabs")),
            },
            Ok(Err(JobError::Runtime(error))) => Err(error),
            Ok(Err(JobError::Cancelled)) => Err(PluginRuntimeError::Cancelled),
            Ok(Err(JobError::Abandoned)) => Err(PluginRuntimeError::EngineUnavailable),
            Ok(Err(JobError::Panicked)) => Err(PluginRuntimeError::Trapped),
            Err(late) => {
                late.cancel();
                Err(PluginRuntimeError::DeadlineExceeded)
            }
        };
        match outcome {
            Ok(tabs) => answered.push((
                package.id().to_owned(),
                Answer {
                    sha256: package.sha256().to_owned(),
                    tabs: Arc::new(tabs),
                },
            )),
            // The runtime has already journaled why. Only a refusal the package
            // would give again is remembered; one the machine caused is not.
            Err(error) => {
                if matches!(PackageRefusal::from_host(error), PackageRefusal::Refused(_)) {
                    answered.push((package.id().to_owned(), refused(package)));
                }
            }
        }
    }
    answered
}

fn refused(package: &StoreTabPackage) -> Answer {
    Answer {
        sha256: package.sha256().to_owned(),
        tabs: Arc::new(Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// The Store's plugin tabs. On a blocking worker, because the first call in a
/// session may compile a component; every later one reads the plugin folder.
#[tauri::command]
pub async fn list_plugin_store_tabs(
    service: tauri::State<'_, Arc<PluginStoreTabs>>,
) -> Result<Vec<StoreTabView>, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.tabs(PluginRuntime::shared))
        .await
        .map_err(|_| "Orivo could not read the Store's plugin tabs.".to_string())
}

/// The WebView names a game it was shown; the address is the host's.
#[tauri::command]
pub fn open_plugin_store_page(
    game_id: String,
    service: tauri::State<'_, Arc<PluginStoreTabs>>,
) -> Result<(), String> {
    service.open_page(&game_id, &crate::store::SystemUrlOpener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_manifest::{
        ArtifactDescriptor, ArtifactKind, PLUGIN_SDK_V1, PluginExtension, PluginManifest,
    };
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        path::Path,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    /// The reference Store tab component (`fixtures/store-tab-fixture`). Its
    /// digest is pinned in `plugin_runtime.rs`'s tests.
    const STORE_FIXTURE: &[u8] = include_bytes!("../fixtures/orivo-store-tab-fixture.wasm");
    const RUNNER_FIXTURE: &[u8] = include_bytes!("../fixtures/orivo-runner-fixture.wasm");
    const PLUGIN_ID: &str = "com.orivo.fixture-store";
    const ALPHA: &str = "com.orivo.fixture-store:alpha";

    fn temporary_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-plugin-store-tabs-{}-{}-{}",
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

    fn sha256_of(bytes: &[u8]) -> String {
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }

    fn install(root: &Path, component: &[u8], extensions: Vec<PluginExtension>) {
        let directory = root.join(PLUGIN_ID);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("component.wasm"), component).unwrap();
        let manifest = PluginManifest {
            id: PLUGIN_ID.into(),
            name: "Fixture Store".into(),
            version: "1.0.0".into(),
            sdk: PLUGIN_SDK_V1.into(),
            min_orivo_version: Some("0.3.0".into()),
            extensions,
            capabilities: Vec::new(),
            network_domains: Vec::new(),
            artifacts: vec![ArtifactDescriptor {
                path: "component.wasm".into(),
                kind: ArtifactKind::Component,
                sha256: sha256_of(component),
                byte_size: component.len() as u64,
            }],
        };
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn service(root: &Path) -> PluginStoreTabs {
        PluginStoreTabs::new(root.to_path_buf(), "0.3.0", true)
    }

    #[derive(Default)]
    struct RecordingOpener(Mutex<Vec<String>>);

    impl UrlOpener for RecordingOpener {
        fn open(&self, url: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(url.to_owned());
            Ok(())
        }
    }

    #[test]
    fn an_installed_store_tab_plugin_lists_its_tabs_and_nothing_it_should_not() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();

        let tabs = service(&root).tabs(|| Ok(runtime.clone()));
        assert_eq!(
            tabs.iter()
                .map(|tab| (
                    tab.plugin_id.as_str(),
                    tab.key.as_deref(),
                    tab.label.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                (PLUGIN_ID, Some("deals"), "Fixture Deals"),
                (PLUGIN_ID, Some("free"), "Free this week"),
            ]
        );
        assert_eq!(tabs[0].accent.as_deref(), Some("#e4572e"));
        assert_eq!(tabs[1].accent, None);
        let alpha = &tabs[0].games[0];
        assert_eq!(alpha.id, ALPHA);
        assert!(alpha.has_page);
        assert_eq!(
            alpha.price,
            Some(StoreTabPriceView {
                currency: "EUR".into(),
                amount_minor: 499,
                regular_amount_minor: Some(1999),
            })
        );
        let beta = &tabs[0].games[1];
        assert!(!beta.has_page);
        assert_eq!(
            (
                beta.cover_url.as_str(),
                beta.hero_url.as_str(),
                beta.logo_url.as_str()
            ),
            ("", "", "")
        );

        // The page stays in the host, and so does where the plugin lives.
        let wire = serde_json::to_string(&tabs).unwrap();
        assert!(!wire.contains("store.example.com"), "{wire}");
        assert!(!wire.contains(root.to_string_lossy().as_ref()));
        fs::remove_dir_all(root).unwrap();
    }

    /// The world imports nothing that varies, so a second visit must not pay for
    /// a second compilation — or run the plugin again.
    #[test]
    fn a_package_is_asked_once_for_the_code_it_is() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);

        let before = PluginRuntime::compiles_on_this_thread();
        let first = service.tabs(|| Ok(runtime.clone()));
        assert_eq!(PluginRuntime::compiles_on_this_thread(), before + 1);
        let second = service.tabs(|| Ok(runtime.clone()));
        assert_eq!(PluginRuntime::compiles_on_this_thread(), before + 1);
        assert_eq!(first, second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_a_listed_game_opens_and_only_at_the_address_the_host_kept() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);
        let opener = RecordingOpener::default();

        // Nothing has been listed yet, so nothing opens.
        assert!(service.open_page(ALPHA, &opener).is_err());
        service.tabs(|| Ok(runtime.clone()));

        service.open_page(ALPHA, &opener).unwrap();
        for refused in [
            "com.orivo.fixture-store:beta",
            "com.orivo.fixture-store:nope",
            "com.other.plugin:alpha",
            "alpha",
            "https://store.example.com/games/alpha",
            "",
        ] {
            assert!(service.open_page(refused, &opener).is_err(), "{refused:?}");
        }
        assert_eq!(
            *opener.0.lock().unwrap(),
            vec!["https://store.example.com/games/alpha".to_string()]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_package_that_goes_away_takes_its_tabs_and_pages_with_it() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);
        assert_eq!(service.tabs(|| Ok(runtime.clone())).len(), 2);
        assert!(service.page_url(ALPHA).is_some());

        fs::remove_dir_all(root.join(PLUGIN_ID)).unwrap();
        assert!(service.tabs(|| Ok(runtime.clone())).is_empty());
        assert_eq!(service.page_url(ALPHA), None);
        fs::remove_dir_all(root).unwrap();
    }

    /// An answer belongs to the code that gave it. Another component under the
    /// same id is asked again — here it no longer keeps the contract, so the
    /// tabs and pages the first one listed are gone.
    #[test]
    fn a_package_that_becomes_other_code_is_asked_again() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);
        assert_eq!(service.tabs(|| Ok(runtime.clone())).len(), 2);

        install(&root, RUNNER_FIXTURE, vec![PluginExtension::StoreTab]);
        assert!(service.tabs(|| Ok(runtime.clone())).is_empty());
        assert_eq!(service.page_url(ALPHA), None);
        fs::remove_dir_all(root).unwrap();
    }

    /// What `lib.rs` does when the installer reports a new package or a removal:
    /// the page stops opening before anyone lists again.
    #[test]
    fn forgetting_a_package_closes_its_pages_at_once() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);
        service.tabs(|| Ok(runtime.clone()));

        service.forget(PLUGIN_ID);
        assert_eq!(service.page_url(ALPHA), None);
        fs::remove_dir_all(root).unwrap();
    }

    /// iOS and the Mac App Store edition run no downloaded code: no tab, no
    /// page, and the plugin engine is never even started for the question.
    #[test]
    fn a_build_without_downloaded_plugins_lists_nothing_and_starts_nothing() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let service = PluginStoreTabs::new(root.clone(), "0.3.0", false);

        let tabs = service.tabs(|| panic!("the runtime was asked for"));
        assert!(tabs.is_empty());
        assert_eq!(service.page_url(ALPHA), None);
        fs::remove_dir_all(root).unwrap();
    }

    /// The extension is the declaration. A package that exports the interface
    /// without naming it at install adds nothing, and one with nothing installed
    /// does not start the engine.
    #[test]
    fn only_a_package_declaring_the_extension_is_asked() {
        let root = temporary_root();
        assert!(service(&root).tabs(|| panic!("nothing to ask")).is_empty());

        install(&root, STORE_FIXTURE, vec![PluginExtension::UiContribution]);
        assert!(service(&root).tabs(|| panic!("not declared")).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// A component that does not keep the Store tab contract has no tab, says
    /// why in the journal, and is not recompiled on the next visit.
    #[test]
    fn a_package_breaking_the_contract_has_no_tab_and_is_not_asked_again() {
        let root = temporary_root();
        install(&root, RUNNER_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);

        let before = PluginRuntime::compiles_on_this_thread();
        assert!(service.tabs(|| Ok(runtime.clone())).is_empty());
        assert!(service.tabs(|| Ok(runtime.clone())).is_empty());
        assert_eq!(PluginRuntime::compiles_on_this_thread(), before + 1);
        assert!(runtime.journal().entries().iter().any(|entry| {
            entry.plugin_id == PLUGIN_ID
                && entry.decision == "store-tabs-refused"
                && entry.detail.contains("Store tab contract")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    /// The scheduler parks a plugin after repeated failures. A parked plugin
    /// shows nothing — even an answer already held — until it is resumed.
    #[test]
    fn a_parked_plugin_has_no_tab_until_it_is_resumed() {
        let root = temporary_root();
        install(&root, STORE_FIXTURE, vec![PluginExtension::StoreTab]);
        let runtime = PluginRuntime::new().unwrap();
        let service = service(&root);
        assert_eq!(service.tabs(|| Ok(runtime.clone())).len(), 2);

        for _ in 0..crate::plugin_runtime::DEFAULT_MAX_CONSECUTIVE_FAILURES {
            let failing = runtime
                .scheduler()
                .submit::<(), _>(PLUGIN_ID, |_| Err(PluginRuntimeError::Trapped))
                .unwrap();
            assert!(failing.wait_for(Duration::from_secs(5)).is_ok());
        }
        assert!(runtime.scheduler().health(PLUGIN_ID).degraded);
        assert!(service.tabs(|| Ok(runtime.clone())).is_empty());
        assert_eq!(service.page_url(ALPHA), None);

        runtime.scheduler().resume(PLUGIN_ID);
        assert_eq!(service.tabs(|| Ok(runtime.clone())).len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
}
