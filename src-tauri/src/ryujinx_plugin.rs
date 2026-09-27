//! The Ryujinx runner plugin, on the real host path.
//!
//! `plugins/ryujinx/` is the first official runner Orivo ships as a WebAssembly
//! component rather than as a native adapter, and the point of this module is
//! that nothing here is a stand-in: the component is the committed artefact
//! `plugins/ryujinx/build.sh` produced, it is installed through
//! `plugin_installer`'s real transaction, configured through
//! `ThirdPartyRunnerService`, and the process at the end is the one
//! `runner_host::prepare_runner_launch` would hand the kernel. Wine and Winlator
//! are Rust in this binary; this one is guest code under the sandbox, so
//! "it works" has to mean the whole chain and not one call.
//!
//! There is no production code in this file. The plugin's own logic lives in
//! `plugins/ryujinx/src/lib.rs`, the host's in `runner_host.rs`; what is written
//! down here is what the two have to agree on. See `docs/ryujinx-runner.md`.
//!
//! The emulation application is always a bundle this module fabricates. Orivo
//! never ships Ryujinx, and no test here installs, downloads or starts it.

use crate::catalog::{Catalog, RunnerProfileStatus};
use crate::plugin_manifest::{
    ArtifactKind, HostCompatibility, PackageEntry, PackageInspection, PackageSignatureStatus,
    PluginCapability, PluginExtension, PluginManifest, validate_plugin_package,
};
use crate::plugin_registry::PluginState;
use crate::plugin_runtime::{
    EpochMode, PluginGrants, PluginLimits, PluginRequest, PluginResponse, PluginRuntime,
};
use crate::plugin_update::PackageChannel;
use crate::runner_commands::ThirdPartyRunnerService;
use crate::runner_host::{
    CatalogStore, RunnerImportLimits, RunnerImportOutcome, prepare_runner_launch, unix_millis,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// The package Orivo ships, read exactly as `cargo test` will always read it:
/// from the committed bytes, with no wasm toolchain in sight.
pub(crate) const COMPONENT: &[u8] = include_bytes!("../../plugins/ryujinx/package/component.wasm");
pub(crate) const MANIFEST_JSON: &str = include_str!("../../plugins/ryujinx/package/manifest.json");
pub(crate) const PLUGIN_ID: &str = "com.orivo.ryujinx";
/// The grant slot the component hard-codes. It is `runner_commands`'
/// `DEFAULT_DIRECTORY_SLOT`, which is the whole reason "Add a folder" needs to
/// know nothing about this plugin.
pub(crate) const GAMES_SLOT: &str = "games";
const PROFILE_ID: &str = "runner-ryujinx-1";

/// Names a real dumped library actually uses. The first three are files the
/// opaque-id grammar cannot spell, which is why the reference is hex; the fourth
/// can be spelled plainly and is here so both forms are exercised by the same
/// import.
const LIBRARY: &[(&str, &str)] = &[
    (
        "Super Mario Odyssey [0100000000010000][v0].nsp",
        "Super Mario Odyssey",
    ),
    (
        "The Legend of Zelda - Tears of the Kingdom [0100F2C0115B6000][v0].xci",
        "The Legend of Zelda - Tears of the Kingdom",
    ),
    // Parentheses stay: a region is part of what tells two dumps apart.
    ("Celeste (USA).nsp", "Celeste (USA)"),
    ("hb-launcher.nro", "hb-launcher"),
];

/// Files a Switch user keeps beside their games, and the ones this plugin must
/// never take an interest in. `prod.keys` is the one that matters: it decrypts
/// every dump the user owns, and it sits in the very folder they allowed.
const NOT_GAMES: &[&str] = &[
    "prod.keys",
    "title.keys",
    "Super Mario Odyssey.sav",
    "cover.jpg",
    "notes.txt",
    "backup.zip",
    // The AppleDouble sidecar macOS writes beside every file on an exFAT volume.
    // It carries the dump's own name and suffix, so nothing but the leading dot
    // tells it apart — and Play would hand its four kilobytes to the emulator.
    "._Super Mario Odyssey [0100000000010000][v0].nsp",
    // Hidden, with a name that is otherwise a perfectly good game.
    ".Celeste (USA).nsp",
];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    root: PathBuf,
    plugin_root: PathBuf,
    games: PathBuf,
    application: PathBuf,
    catalog_path: PathBuf,
    service: Arc<ThirdPartyRunnerService>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn new(tag: &str) -> Self {
        Self::with_library(tag, LIBRARY, RunnerImportLimits::default())
    }

    fn with_library(tag: &str, library: &[(&str, &str)], limits: RunnerImportLimits) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-ryujinx-plugin-{tag}-{}-{}-{}",
            std::process::id(),
            unix_millis(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let games = root.join("games");
        fs::create_dir_all(&games).unwrap();
        for (name, _) in library {
            // Bytes that are not a dump: nothing here may depend on a real one,
            // and a test fixture that looked like a game would be a game.
            fs::write(games.join(name), b"not a Nintendo Switch dump").unwrap();
        }
        for name in NOT_GAMES {
            fs::write(games.join(name), b"private").unwrap();
        }
        // A folder, to prove a directory entry is not offered as a game even
        // when it is named like one.
        fs::create_dir_all(games.join("firmware.nsp")).unwrap();

        let plugin_root = root.join("plugins");
        let application = fake_application(&root);
        let catalog_path = root.join("catalog.json");
        let harness = Self {
            service: Self::service(&plugin_root, &catalog_path, Catalog::default(), limits),
            root,
            plugin_root,
            games,
            application,
            catalog_path,
        };
        harness.install();
        harness
    }

    fn service(
        plugin_root: &Path,
        catalog_path: &Path,
        catalog: Catalog,
        limits: RunnerImportLimits,
    ) -> Arc<ThirdPartyRunnerService> {
        let store = CatalogStore::new(
            Arc::new(RwLock::new(catalog)),
            catalog_path.to_path_buf(),
            Arc::new(Mutex::new(())),
        );
        Arc::new(
            ThirdPartyRunnerService::new(
                store,
                plugin_root.to_path_buf(),
                HostCompatibility::v1(env!("CARGO_PKG_VERSION")),
            )
            // Its own engine, so a bounded queue is not shared with every other
            // test in this binary.
            .with_runtime(
                PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Threaded).unwrap(),
            )
            .with_import_limits(limits),
        )
    }

    /// Install the shipped package the way a user would today: as a
    /// `.orivo-plugin` archive on the development channel, through the real
    /// transaction. There is no release signature to present — who holds the
    /// registry key is an open question, see `docs/ryujinx-runner.md` — so this
    /// is the channel the plugin genuinely arrives on.
    fn install(&self) {
        let installer = crate::plugin_installer::PluginInstallerService::new(
            self.plugin_root.clone(),
            env!("CARGO_PKG_VERSION"),
        );
        let files = crate::plugin_installer::read_package(&package_archive()).unwrap();
        crate::plugin_installer::install_verified(
            &installer,
            PLUGIN_ID,
            manifest().version.as_str(),
            PackageChannel::Development,
            &files,
            // A hand-loaded package on the development channel, which is the door
            // with no downgrade rule to re-check: `expected` is only `Some` through
            // the registry.
            false,
        )
        .expect("the shipped Ryujinx package must install");
    }

    /// The flow "Add an emulator" drives: pick the application, then allow one
    /// folder.
    fn configure(&self) {
        let profile = self
            .service
            .create_profile_with_id(PROFILE_ID, PLUGIN_ID, "Ryujinx", &self.application)
            .unwrap();
        assert_eq!(
            profile.status,
            RunnerProfileStatus::Valid,
            "{:?}",
            profile.status_message
        );
        self.service
            .grant_directory(PROFILE_ID, Some(GAMES_SLOT), &self.games)
            .unwrap();
    }

    fn import(&self) -> RunnerImportOutcome {
        let cancelled = AtomicBool::new(false);
        self.service
            .import_now(PROFILE_ID, &cancelled, |_| {})
            .unwrap()
    }

    fn catalog(&self) -> Catalog {
        Catalog::load(&self.catalog_path).unwrap()
    }

    /// A second service over the same catalog file, which is what a restart is.
    fn restart(&self, limits: RunnerImportLimits) -> Arc<ThirdPartyRunnerService> {
        Self::service(
            &self.plugin_root,
            &self.catalog_path,
            Catalog::load(&self.catalog_path).unwrap(),
            limits,
        )
    }

    /// One card, by the title the plugin proposed.
    fn card(&self, title: &str) -> crate::catalog::Game {
        self.catalog()
            .games
            .iter()
            .find(|game| game.title == title)
            .cloned()
            .unwrap_or_else(|| panic!("no card titled {title:?}"))
    }
}

pub(crate) fn manifest() -> PluginManifest {
    serde_json::from_str(MANIFEST_JSON).expect("the shipped manifest must parse")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

/// The shipped package as the archive the installer reads, built from the same
/// two committed files rather than from values restated here.
pub(crate) fn package_archive() -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    let files: [(&str, Vec<u8>); 2] = [
        ("manifest.json", MANIFEST_JSON.as_bytes().to_vec()),
        ("component.wasm", COMPONENT.to_vec()),
    ];
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (path, contents) in &files {
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

/// A macOS application bundle shaped like Ryujinx's own — `Contents/Info.plist`
/// naming `Contents/MacOS/<CFBundleExecutable>` — with this test binary as the
/// executable inside it.
///
/// The bundle matters because that is what the native picker returns on macOS:
/// the user chooses `Ryujinx.app`, a directory, and the host has to find the
/// program inside it. A real executable rather than a script, for the reason
/// `runner_commands`' own fixture gives: the host starts a process with no
/// interpreter in between, and started with a path as its only argument libtest
/// reads that as a filter, matches nothing and exits successfully.
pub(crate) fn fake_application(root: &Path) -> PathBuf {
    let bundle = root.join("Test Emulator.app");
    let macos = bundle.join("Contents/MacOS");
    fs::create_dir_all(&macos).unwrap();
    fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>Test Emulator</string>
  <key>CFBundleIdentifier</key>
  <string>com.orivo.test-emulator</string>
  <key>CFBundleName</key>
  <string>Test Emulator</string>
</dict>
</plist>
"#,
    )
    .unwrap();
    let executable = macos.join("Test Emulator");
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bundle
}

/// The hex reference the component issues for one file name. Written here the
/// long way rather than imported, because the encoding is the contract between
/// two separately built artefacts: if the host's decoder and the component's
/// encoder ever disagree, this is the line that has to be wrong for the test to
/// pass.
pub(crate) fn reference(name: &str) -> String {
    let digits: String = name
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("x:{digits}")
}

/// The same reference with its digits upper-cased, which `char::to_digit(16)`
/// used to accept on both sides. One file has to have one reference: a card is
/// keyed by it, so a second spelling would have been a second card.
fn shouted(reference: &str) -> String {
    let (prefix, digits) = reference.split_at(2);
    format!("{prefix}{}", digits.to_ascii_uppercase())
}

/// One call straight through `PluginRuntime`, for the questions only the host's
/// own accounting can answer — how much the component read, how many host calls
/// it made.
fn invoke(harness: &Harness, request: PluginRequest) -> crate::plugin_runtime::PluginInvocation {
    let runtime = PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Threaded).unwrap();
    let prepared = runtime
        .prepare_component(COMPONENT, &sha256_hex(COMPONENT))
        .unwrap();
    let validated = manifest().validate().unwrap();
    let grant = crate::plugin_manifest::CapabilityGrant {
        plugin_id: PLUGIN_ID.into(),
        capability: PluginCapability::FilesRead,
        scope: crate::plugin_manifest::CapabilityScope::DirectoryGrants(
            [GAMES_SLOT.to_string()].into_iter().collect(),
        ),
    };
    let directories: BTreeMap<String, PathBuf> = [(GAMES_SLOT.to_string(), harness.games.clone())]
        .into_iter()
        .collect();
    let grants = PluginGrants::resolve(&validated, &[grant], &directories).unwrap();
    runtime
        .submit(&prepared, PLUGIN_ID, &grants, request)
        .unwrap()
        .wait_for(Duration::from_secs(10))
        .expect("the call must finish")
        .expect("the call must succeed")
}

// ---------------------------------------------------------------------------
// The package Orivo ships
// ---------------------------------------------------------------------------

/// A stale digest fails here first, which is the point: every assertion below is
/// only about the plugin `plugins/ryujinx/src/lib.rs` describes if the committed
/// component is the one that source built.
#[test]
fn the_committed_component_is_the_artefact_its_manifest_declares() {
    let manifest = manifest();
    let component = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == ArtifactKind::Component)
        .expect("the manifest must declare a component");
    assert_eq!(component.path, "component.wasm");
    assert_eq!(
        component.sha256,
        sha256_hex(COMPONENT),
        "run plugins/ryujinx/build.sh and paste the digest it prints"
    );
    assert_eq!(component.byte_size, COMPONENT.len() as u64);
    assert_eq!(manifest.id, PLUGIN_ID);
    assert_eq!(manifest.extensions, vec![PluginExtension::Runner]);
    // Two capabilities and no more. `network_fetch` and `secrets` are the two a
    // runner has no business holding, and the manifest is where a user sees
    // that.
    assert_eq!(
        manifest.capabilities,
        vec![PluginCapability::RunnerPrepare, PluginCapability::FilesRead]
    );
    assert!(manifest.network_domains.is_empty());
}

/// The same rules the installer applies to anybody's archive, applied to ours.
#[test]
fn the_shipped_package_passes_the_hosts_own_package_rules() {
    let inspection = PackageInspection {
        entries: vec![
            PackageEntry {
                path: "manifest.json".into(),
                byte_size: MANIFEST_JSON.len() as u64,
            },
            PackageEntry {
                path: "component.wasm".into(),
                byte_size: COMPONENT.len() as u64,
            },
        ],
        // What this plugin genuinely arrives as: a development-channel package,
        // because Orivo's release key is not this repository's to use.
        signature: PackageSignatureStatus::Development,
    };
    let validated = validate_plugin_package(manifest(), &inspection)
        .unwrap_or_else(|errors| panic!("{:?}", errors.0));
    assert_eq!(validated.manifest.id(), PLUGIN_ID);
}

/// Discovery judges a runner by its component, not by its manifest: the package
/// has to export the runner world, import nothing the host cannot serve, ask for
/// no more than it declared, and answer `get-identity` with an identity that
/// agrees with the package installed.
#[test]
fn the_installed_plugin_is_offered_as_a_configurable_runner() {
    let harness = Harness::new("discovery");
    let runners = harness.service.installed_runners().unwrap();
    let ryujinx = runners
        .iter()
        .find(|runner| runner.id == PLUGIN_ID)
        .expect("the installed plugin must be discovered");
    assert_eq!(ryujinx.state, PluginState::Ready, "{}", ryujinx.message);
    assert_eq!(ryujinx.name, "Ryujinx");
    assert_eq!(ryujinx.version, "1.0.0");
    assert!(ryujinx.profiles.is_empty());
}

/// Settings' "Add a game folder…" passes no slot (`runner-view.ts` calls
/// `grantDirectory(profile.id)`), so the command falls back to
/// `DEFAULT_DIRECTORY_SLOT` — and this component hard-codes the slot it asks
/// `host-files` for, because the v1 manifest has no field to declare one in. The
/// two have to be the same string or that button silently grants a folder the
/// plugin cannot read, and a comment saying so is not a guarantee.
///
/// So this configures a profile the way E3 does — without naming a slot at all —
/// and imports through it.
#[test]
fn the_folder_button_in_settings_grants_the_slot_this_component_asks_for() {
    assert_eq!(GAMES_SLOT, crate::runner_commands::DEFAULT_DIRECTORY_SLOT);

    let harness = Harness::new("default-slot");
    harness
        .service
        .create_profile_with_id(PROFILE_ID, PLUGIN_ID, "Ryujinx", &harness.application)
        .unwrap();
    let profile = harness
        .service
        .grant_directory(PROFILE_ID, None, &harness.games)
        .unwrap();
    assert_eq!(profile.directories.len(), 1);
    assert!(profile.directories[0].granted);

    let outcome = harness.import();
    assert_eq!(outcome.progress.imported, LIBRARY.len());
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// The whole import, on names a real dumped library uses. Three of the four do
/// not pass the opaque-id grammar, so they are games the host could not have
/// resolved before the hex reference existed.
#[test]
fn a_realistically_named_switch_library_imports_with_readable_titles() {
    let harness = Harness::new("import");
    harness.configure();
    let outcome = harness.import();

    assert_eq!(outcome.progress.imported, LIBRARY.len());
    assert!(outcome.complete);
    let catalog = harness.catalog();
    assert_eq!(catalog.games.len(), LIBRARY.len());
    for (name, title) in LIBRARY {
        let card = harness.card(title);
        let entry = catalog
            .runner_inventory
            .iter()
            .find(|entry| entry.game_ref == reference(name))
            .unwrap_or_else(|| panic!("{name} was not imported"));
        assert_eq!(entry.title, *title);
        assert_eq!(entry.game_path.file_name().unwrap(), *name);
        assert_eq!(entry.directory_grant_id, GAMES_SLOT);
        // A card carries opaque references and no path: the plugin's own file
        // never reaches the library row, let alone the WebView.
        assert!(card.executable_path.is_none());
        assert!(matches!(
            card.launch_target,
            crate::catalog::LaunchTarget::Runner { .. }
        ));
    }
}

/// The title id is the one fact a dump's file name carries that is worth
/// keeping, and the v1 `game-candidate` has nowhere to put it except `platform`,
/// which lands on the card's own metadata line.
#[test]
fn a_title_id_in_the_file_name_travels_onto_the_card() {
    let harness = Harness::new("title-id");
    harness.configure();
    harness.import();

    assert_eq!(
        harness.card("Super Mario Odyssey").metadata.as_deref(),
        Some("Nintendo Switch · 0100000000010000")
    );
    // Upper-cased, because a title id is conventionally written that way and two
    // spellings of one id would search as two games.
    assert_eq!(
        harness
            .card("The Legend of Zelda - Tears of the Kingdom")
            .metadata
            .as_deref(),
        Some("Nintendo Switch · 0100F2C0115B6000")
    );
    // No id in the name, so nothing is invented for it.
    assert_eq!(
        harness.card("Celeste (USA)").metadata.as_deref(),
        Some("Nintendo Switch")
    );
}

/// The library folder is also where a user keeps the keys that decrypt every
/// dump they own. Nothing here is offered as a game, and — the assertion that
/// actually matters — the host's own byte counter says the component did not
/// read one.
#[test]
fn nothing_beside_the_games_is_offered_and_no_file_is_ever_read() {
    let harness = Harness::new("no-reads");
    harness.configure();
    harness.import();

    let catalog = harness.catalog();
    assert_eq!(catalog.runner_inventory.len(), LIBRARY.len());
    for name in NOT_GAMES {
        assert!(
            !catalog
                .runner_inventory
                .iter()
                .any(|entry| entry.game_path.file_name().unwrap() == *name),
            "{name} must not become a game"
        );
    }
    // A directory called `firmware.nsp` has the right extension and is still not
    // a file.
    assert!(
        !catalog
            .games
            .iter()
            .any(|game| game.title.contains("firmware"))
    );

    let invocation = invoke(
        &harness,
        PluginRequest::DiscoverPage {
            profile_id: PROFILE_ID.into(),
            cursor: None,
            limit: 50,
        },
    );
    assert_eq!(
        invocation.cost.bytes_read, 0,
        "this plugin lists a folder and never opens a file in it"
    );
    match invocation.response {
        PluginResponse::DiscoveryPage(page) => assert_eq!(page.games.len(), LIBRARY.len()),
        other => panic!("expected a discovery page, got {other:?}"),
    }
}

/// The AppleDouble sidecar. On any exFAT or FAT volume — which is what a shared
/// ROM drive usually is — macOS writes `._<name>` beside every file, carrying the
/// dump's own name and `.nsp` suffix. Nothing but the leading dot tells the two
/// apart, so it would earn a second card with the same title and the same title
/// id, and Play would hand its four kilobytes to the emulator.
///
/// Ryujinx never sees one: its `EnumerationOptions` leaves `AttributesToSkip` at
/// the default `Hidden | System`. `host-files` has no such default, so both the
/// plugin and the host apply the rule themselves.
#[test]
fn a_hidden_file_never_becomes_a_card_or_a_launch() {
    let harness = Harness::new("hidden");
    harness.configure();
    harness.import();

    let catalog = harness.catalog();
    let sidecar = "._Super Mario Odyssey [0100000000010000][v0].nsp";
    assert_eq!(
        catalog
            .games
            .iter()
            .filter(|game| game.title == "Super Mario Odyssey")
            .count(),
        1,
        "the sidecar must not earn a second card under the dump's own title"
    );
    assert!(
        !catalog
            .runner_inventory
            .iter()
            .any(|entry| entry.game_path.file_name().unwrap() == sidecar)
    );
    // And it cannot be reached by naming it either: a plain id could never start
    // with a dot, and the hex form must not be the way around that.
    let package = harness.service.package(PLUGIN_ID).unwrap();
    let cancelled = AtomicBool::new(false);
    assert!(
        prepare_runner_launch(
            &package,
            &catalog,
            PROFILE_ID,
            &reference(sidecar),
            &cancelled,
        )
        .is_err()
    );
}

/// A file with nothing to call it must cost that one file, not the library.
///
/// The host refuses a candidate whose title is blank and refuses the whole page
/// with it (`plugin_runtime`'s `sanitise_text`), and an import retries the same
/// page every time — so a single ` .nsp`, or a name made of a no-break space or
/// U+3000, used to stop every game behind it. It sorts first, too, so "behind it"
/// meant all of them.
#[test]
fn a_name_with_no_title_in_it_costs_one_card_and_not_the_page() {
    const AWKWARD: &[(&str, &str)] = &[
        // Blank before the extension, three ways: ASCII space, no-break space,
        // and the ideographic space.
        (" .nsp", ".nsp"),
        ("\u{a0}.xci", ".xci"),
        ("\u{3000}.nsp", ".nsp"),
        // Nothing left once the brackets come off, so the stem is the label.
        ("[0100000000010000].nsp", "[0100000000010000]"),
        // And a real game, which is what must survive all of the above.
        (
            "Super Mario Odyssey [0100000000010000][v0].nsp",
            "Super Mario Odyssey",
        ),
    ];
    let harness = Harness::with_library("awkward", AWKWARD, RunnerImportLimits::default());
    harness.configure();
    let outcome = harness.import();

    assert!(outcome.complete);
    assert_eq!(
        outcome.progress.imported,
        AWKWARD.len(),
        "one unnameable file must not refuse the page the rest of the library is on"
    );
    let catalog = harness.catalog();
    for (name, title) in AWKWARD {
        let entry = catalog
            .runner_inventory
            .iter()
            .find(|entry| entry.game_ref == reference(name))
            .unwrap_or_else(|| panic!("{name:?} was not imported"));
        assert_eq!(entry.title, *title, "for {name:?}");
    }
}

/// A library bigger than one page is walked in pages with a cursor, and a
/// restart continues from the cursor rather than from the beginning. This is the
/// plan's own exit test — "a runner can import a game and relaunch it after a
/// restart without rescanning the whole library" — for this plugin's cursor,
/// which is a position in a sorted listing rather than a name.
#[test]
fn a_library_larger_than_one_page_is_imported_in_pages_and_resumes() {
    let harness = Harness::with_library(
        "paging",
        LIBRARY,
        RunnerImportLimits {
            page_size: 2,
            max_pages: 1,
            max_games: 20_000,
        },
    );
    harness.configure();

    let first = harness.import();
    assert_eq!(first.progress.pages, 1);
    assert_eq!(first.progress.imported, 2);
    assert!(!first.complete);
    let cursor = harness
        .catalog()
        .runner_profile(PROFILE_ID)
        .unwrap()
        .import_cursor
        .clone()
        .expect("an unfinished import must leave a cursor behind");

    let resumed = harness.restart(RunnerImportLimits {
        page_size: 2,
        max_pages: 8,
        max_games: 20_000,
    });
    let cancelled = AtomicBool::new(false);
    let outcome = resumed.import_now(PROFILE_ID, &cancelled, |_| {}).unwrap();
    assert_eq!(outcome.resumed_from.as_deref(), Some(cursor.as_str()));
    assert_eq!(outcome.progress.imported, LIBRARY.len() - 2);
    // Nothing was imported twice: the second run refreshed nothing, which is
    // what "did not rescan the library" means in a number.
    assert_eq!(outcome.progress.refreshed, 0);
    assert!(outcome.complete);
    assert_eq!(harness.catalog().runner_inventory.len(), LIBRARY.len());
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// The process, which is the whole security claim: the program is the executable
/// the host found inside the bundle the user picked, the single argument is the
/// file the host resolved inside the folder the user allowed, and no shell,
/// interpreter or plugin-supplied string appears anywhere.
#[test]
fn the_process_is_the_bundle_executable_with_the_rom_as_its_only_argument() {
    let harness = Harness::new("launch");
    harness.configure();
    harness.import();

    let package = harness.service.package(PLUGIN_ID).unwrap();
    let cancelled = AtomicBool::new(false);
    let (name, title) = LIBRARY[0];
    let prepared = prepare_runner_launch(
        &package,
        &harness.catalog(),
        PROFILE_ID,
        &reference(name),
        &cancelled,
    )
    .unwrap();
    assert_eq!(prepared.title(), title);

    let command = prepared.command();
    let inside_bundle = fs::canonicalize(
        harness
            .application
            .join("Contents/MacOS")
            .join("Test Emulator"),
    )
    .unwrap();
    assert_eq!(
        command.get_program(),
        inside_bundle.as_os_str(),
        "the bundle is a directory; the program has to be the binary inside it"
    );
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            fs::canonicalize(harness.games.join(name))
                .unwrap()
                .as_os_str()
        ],
        "one argument, and it is the resolved game file"
    );

    // And it really starts: no shell, no interpreter, the child is the program
    // the host resolved.
    #[cfg(unix)]
    {
        let mut child = prepared.spawn().unwrap();
        assert!(child.wait().unwrap().success());
    }
}

/// The plugin's own refusal, before the host's. A reference that is not one this
/// component could have issued names no Switch game file, and saying so costs no
/// directory scan — `prepare-launch` runs under the interactive budget.
#[test]
fn prepare_launch_refuses_a_reference_this_plugin_never_issued() {
    let runtime = PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Threaded).unwrap();
    let prepared = runtime
        .prepare_component(COMPONENT, &sha256_hex(COMPONENT))
        .unwrap();
    let grants = PluginGrants::declared_only(&manifest().validate().unwrap());

    for reference in [
        // Not hex at all.
        "not-a-reference",
        // Hex, and of a real file, but not one with an extension Ryujinx opens:
        // the refusal is the extension, not the path shape.
        reference("prod.keys").as_str(),
        // A traversal that *does* end in a suffix Ryujinx opens, so only the
        // decoded name's shape can refuse it. The host would refuse it too — no
        // entry is called this — but the plugin declines to say it at all.
        reference("../Celeste (USA).nsp").as_str(),
        // Hidden, and the plugin never offered it, so it will not prepare it.
        reference("._Celeste (USA).nsp").as_str(),
        // The right bytes in the wrong case: one file, one reference.
        shouted(&reference("Celeste (USA).nsp")).as_str(),
        // The prefix alone, and a plain name with no prefix at all.
        "x:",
        "Celeste.nsp",
    ] {
        let outcome = runtime
            .submit(
                &prepared,
                PLUGIN_ID,
                &grants,
                PluginRequest::PrepareLaunch {
                    profile_id: PROFILE_ID.into(),
                    game_reference: reference.to_string(),
                },
            )
            .unwrap()
            .wait_for(Duration::from_secs(10))
            .expect("the call must finish");
        assert!(
            outcome.is_err(),
            "{reference} should not become a launch intent"
        );
        // Every refusal here is the plugin declining, which the scheduler counts
        // against it: three in a row is `degraded`, and the fourth would never be
        // dispatched. That is the scheduler behaving correctly, so the test
        // clears it between references rather than working around it.
        runtime.scheduler().resume(PLUGIN_ID);
    }
}
