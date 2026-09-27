//! O1 — reference numbers for `docs/performance.md`.
//!
//! Every function here is `#[ignore]`: none of it runs in `cargo test`, because
//! a wall-clock assertion in the normal gate is a flake waiting for a loaded
//! CI runner or a laptop under thermal pressure. Run it by hand and read the
//! numbers instead:
//!
//! ```sh
//! cargo test --manifest-path src-tauri/Cargo.toml --release perf_bench -- --ignored --nocapture
//! ```
//!
//! `--release` matters more here than in most Rust tests: `AppState::load` and
//! the plugin host both run at release optimisation in the shipped app, and
//! the dev profile's JSON parsing and Wasmtime compilation are several times
//! slower in a way that would misrepresent the budget in `docs/performance.md`.
//!
//! This module measures, it does not optimise — that split is deliberate, see
//! docs/plugin-system-plan.md's "Étape 2.4". Three questions, matching what
//! `AppState::load` (src-tauri/src/lib.rs) actually does at startup and what
//! the Settings › Plugins panel and the emulator flow ask for on demand:
//!
//! 1. Does the catalog step of startup scale acceptably from a hobby library
//!    (10 games) to a hoarder's (10,000)? `catalog::Catalog::load_with_migration`
//!    and `save_atomically` are exactly what `load_or_migrate_catalog` calls.
//! 2. Is the Wine auto-apply pass's own cost — a disk probe once, then a
//!    canonicalise-and-hash per pending `.exe` — still linear rather than
//!    quadratic in catalog size? It was measured as part of startup itself
//!    until O2b took it off that path, mirroring what M1 did for Winlator's
//!    adoption pass: both now run in the background, after the first paint.
//!    `bench_catalog_at_size` no longer times it at all — see its doc comment
//!    — and `bench_wine_auto_apply_at_size` measures it standalone instead.
//! 3. What does having plugins installed cost the two on-demand commands that
//!    actually touch the plugin runtime — `get_plugin_catalog` (Settings ›
//!    Plugins) and `get_runner_plugins` (the emulator flow) — neither of which
//!    sits on the startup, navigation, rail or search path? `plugin_runtime.rs`
//!    already has `reports_what_a_prepared_component_costs_against_a_cold_one`
//!    for the cost of a single cold component; this module multiplies that by
//!    plugin count instead of re-measuring it.

use crate::catalog::{CURRENT_SCHEMA_VERSION, Catalog};
use crate::plugin_compile_cache::{CacheLimits, ComponentCache};
use crate::plugin_manifest::{
    ArtifactDescriptor, ArtifactKind, HostCompatibility, PLUGIN_SDK_V1, PluginCapability,
    PluginExtension, PluginManifest,
};
use crate::plugin_registry::PluginRegistry;
use crate::plugin_runtime::PluginRuntime;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const WARMUP_ITERATIONS: usize = 1;
const MEASURED_ITERATIONS: usize = 5;

/// The real fixture from P1/P2 (`docs/plugin-system-plan.md` step 1.3), not a
/// stub: a compile-cost number measured against eight zero bytes would not
/// tell docs/performance.md anything about a real third-party component.
const RUNNER_COMPONENT: &[u8] = include_bytes!("../fixtures/orivo-runner-fixture.wasm");
const RUNNER_ID: &str = "com.orivo.fixture-runner";

/// A root no other test run can collide with, mirroring the same pattern in
/// `plugin_registry.rs`'s own tests — the clock alone is not enough once runs
/// overlap.
fn scratch_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "orivo-perf-bench-{label}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// min / median / max over `MEASURED_ITERATIONS` runs, printed in a
/// grep-friendly shape. The spread matters as much as the median: a wide
/// spread on a supposedly pure-CPU step usually means a syscall or an
/// allocator stall hiding inside it.
fn report(label: &str, n: usize, mut samples: Vec<Duration>) {
    samples.sort();
    let min = samples[0];
    let median = samples[samples.len() / 2];
    let max = samples[samples.len() - 1];
    println!("PERF {label:<40} n={n:<6} min={min:>10.2?} median={median:>10.2?} max={max:>10.2?}");
}

fn timed<T>(mut work: impl FnMut() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = work();
    (value, start.elapsed())
}

/// A `catalog.json` body with `count` games, a fraction of them local `.exe`
/// Direct games so `wine_auto_apply_candidates`'s filter has something to walk
/// instead of short-circuiting on an empty catalog. Built as text, not as
/// `Game` values: this is exactly the shape `load_or_migrate_catalog` reads
/// off disk, and it exercises the same `#[serde(default)]` fill-in a real
/// catalog file relies on.
///
/// Every game needs an `executable_path`: `Catalog::validate` (called from
/// `save_atomically`) rejects a `Local` + `Direct` game — the default for both
/// fields — that has none.
fn synthetic_catalog_json(count: usize) -> String {
    let mut games = String::with_capacity(count * 96);
    for index in 0..count {
        if index > 0 {
            games.push(',');
        }
        // Every tenth game is a local Windows executable, so the Wine
        // auto-apply pass has real candidates instead of an early return; the
        // rest are a non-`.exe` local game, e.g. a native macOS/Linux binary.
        let executable = if index % 10 == 0 {
            format!("/bench/library/game-{index}/game.exe")
        } else {
            format!("/bench/library/game-{index}/game")
        };
        games.push_str(&format!(
            r#"{{"id":"bench-{index}","title":"Bench Game {index}","executable_path":"{executable}"}}"#
        ));
    }
    format!(
        r#"{{"schema_version":{CURRENT_SCHEMA_VERSION},"games":[{games}],"wine_profiles":[],"wine_inventory":[],"winlator_profiles":[],"winlator_inventory":[]}}"#
    )
}

/// `load_with_migration` + `save_atomically`, at one catalog size. This is the
/// catalog sequence `AppState::load` runs on every startup.
///
/// The Wine auto-apply pass measured here too until O2b (see
/// `bench_wine_auto_apply_at_size` below) took it off the startup path
/// entirely, the same way M1 (#44) took Winlator's own adoption pass off it:
/// it now runs in the background, after the first paint, in bounded pages —
/// so, like the Winlator row this note replaces, `AppState::load` no longer
/// pays for it at all, and this bench no longer measures it as part of
/// startup.
fn bench_catalog_at_size(n: usize) {
    let dir = scratch_dir(&format!("catalog-{n}"));
    let catalog_path = dir.join("catalog.json");
    fs::write(&catalog_path, synthetic_catalog_json(n)).unwrap();

    for _ in 0..WARMUP_ITERATIONS {
        let _ = Catalog::load_with_migration(&catalog_path).unwrap();
    }

    let mut load_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut save_samples = Vec::with_capacity(MEASURED_ITERATIONS);

    for _ in 0..MEASURED_ITERATIONS {
        let (loaded, load_elapsed) = timed(|| Catalog::load_with_migration(&catalog_path).unwrap());
        load_samples.push(load_elapsed);

        let catalog = loaded.catalog;
        let save_path = dir.join("catalog.save.json");
        let (_, save_elapsed) = timed(|| catalog.save_atomically(&save_path).unwrap());
        save_samples.push(save_elapsed);
    }

    report("catalog.load_with_migration", n, load_samples);
    report("catalog.save_atomically", n, save_samples);

    fs::remove_dir_all(&dir).ok();
}

#[test]
#[ignore]
fn bench_catalog_10_games() {
    bench_catalog_at_size(10);
}

#[test]
#[ignore]
fn bench_catalog_1000_games() {
    bench_catalog_at_size(1_000);
}

#[test]
#[ignore]
fn bench_catalog_10000_games() {
    bench_catalog_at_size(10_000);
}

/// The Wine auto-apply pass, on its own, off the startup path it used to sit
/// on. Same synthetic catalog as `bench_catalog_at_size`, but timed apart from
/// `load_with_migration`/`save_atomically` because nothing in `AppState::load`
/// calls it any more (see that function's doc comment, and O2b): it now runs
/// from `spawn_wine_auto_apply`, once the shell is on screen, one bounded page
/// at a time.
///
/// No managed profile exists yet in this synthetic catalog, so every run pays
/// `wine_runner::detect_wine_staging`'s fixed disk probe — the same cost
/// `docs/performance.md` recorded for the old startup call, reproduced here to
/// show moving it did not change what it costs, only when it runs.
fn bench_wine_auto_apply_at_size(n: usize) {
    let dir = scratch_dir(&format!("wine-auto-apply-{n}"));
    let catalog_path = dir.join("catalog.json");
    fs::write(&catalog_path, synthetic_catalog_json(n)).unwrap();
    let wine_prefix_root = dir.join("wine-prefixes");

    let mut wine_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let mut catalog = Catalog::load_with_migration(&catalog_path).unwrap().catalog;
        let candidates = crate::wine_auto_apply_candidates(&catalog);
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let (_, wine_elapsed) = timed(|| {
            crate::apply_wine_auto_apply_to_candidates(
                &mut catalog,
                &wine_prefix_root,
                &candidates,
                &cancelled,
            )
        });
        wine_samples.push(wine_elapsed);
    }

    report("wine_auto_apply (background)", n, wine_samples);

    fs::remove_dir_all(&dir).ok();
}

#[test]
#[ignore]
fn bench_wine_auto_apply_10_games() {
    bench_wine_auto_apply_at_size(10);
}

#[test]
#[ignore]
fn bench_wine_auto_apply_1000_games() {
    bench_wine_auto_apply_at_size(1_000);
}

#[test]
#[ignore]
fn bench_wine_auto_apply_10000_games() {
    bench_wine_auto_apply_at_size(10_000);
}

/// Whether the installed copies share one component or carry different bytes.
///
/// `Identical` is what the numbers in `docs/performance.md` section 3 were taken
/// with, and it is kept so those stay comparable. It is the wrong shape for a
/// compile cache: N copies of one component are N packages sharing *one*
/// artifact, which flatters a cache that is meant to hold one per component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Components {
    Identical,
    Distinct,
}

/// The fixture with a WebAssembly custom section appended, so every copy is a
/// different component with a different digest.
///
/// A custom section is ignored by the component model and by Cranelift, which is
/// what makes this a fair stand-in for N unrelated third-party runners: each one
/// compiles to the same amount of code and lands in its own cache slot, so the
/// cached measurement is N loads of N artifacts rather than N loads of one.
fn fixture_variant(index: usize) -> Vec<u8> {
    const SECTION_NAME: &[u8] = b"orivo-bench";
    let mut payload = Vec::with_capacity(1 + SECTION_NAME.len() + 8);
    payload.push(SECTION_NAME.len() as u8);
    payload.extend_from_slice(SECTION_NAME);
    payload.extend_from_slice(&(index as u64).to_le_bytes());

    let mut bytes = Vec::with_capacity(RUNNER_COMPONENT.len() + payload.len() + 2);
    bytes.extend_from_slice(RUNNER_COMPONENT);
    bytes.push(0x00);
    bytes.push(payload.len() as u8);
    bytes.extend_from_slice(&payload);
    bytes
}

/// Installs `count` copies of the real runner fixture, each under its own
/// directory.
///
/// `PluginRegistry::inspect_plugin_directory` refuses a package outright —
/// `component: None`, no compile ever attempted — when the directory name
/// does not equal the manifest's own id ("The plugin folder does not match
/// its manifest identity"). An earlier version of this bench reused the
/// fixture's real id (`com.orivo.fixture-runner`) for every directory, which
/// only satisfied that check for the first copy; the rest were rejected
/// before Wasmtime ever saw their bytes, so the "N plugins" numbers were
/// actually "1 plugin plus N-1 free directory-name rejections". Giving each
/// copy its own id (and matching directory name) gets all of them into
/// `preflight`, which is what pays the real per-component compile cost this
/// bench exists to show. The fixture's *compiled-in* identity is still the
/// same for every copy, so `get-identity` disagrees with the manifest from
/// the second copy on — full compile and instantiate cost, `Invalid` result
/// — which is a fair stand-in for N distinct, unrelated third-party runners.
fn install_fixture_copies(
    root: &Path,
    count: usize,
    extensions: Vec<PluginExtension>,
    components: Components,
) {
    for index in 0..count {
        let id = if index == 0 {
            RUNNER_ID.to_string()
        } else {
            format!("com.orivo.bench-runner-{index}")
        };
        let directory = root.join(&id);
        fs::create_dir_all(&directory).unwrap();
        let component = match components {
            Components::Identical => RUNNER_COMPONENT.to_vec(),
            Components::Distinct => fixture_variant(index),
        };
        fs::write(directory.join("component.wasm"), &component).unwrap();
        let manifest = PluginManifest {
            id,
            name: "Fixture Runner".into(),
            version: "1.0.0".into(),
            sdk: PLUGIN_SDK_V1.into(),
            min_orivo_version: Some("0.3.0".into()),
            extensions: extensions.clone(),
            capabilities: vec![PluginCapability::RunnerPrepare, PluginCapability::FilesRead],
            network_domains: Vec::new(),
            artifacts: vec![ArtifactDescriptor {
                path: "component.wasm".into(),
                kind: ArtifactKind::Component,
                sha256: sha256_hex(&component),
                byte_size: component.len() as u64,
            }],
        };
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }
}

/// `get_plugin_catalog` (Settings › Plugins) and `get_runner_plugins` (the
/// emulator flow) both call `PluginRuntime::shared()`, the process-wide
/// engine `AppState::load` never touches — building it once here, outside the
/// timed loop, is what makes the numbers below "cost of discovering N
/// plugins" and not "cost of discovering N plugins plus starting Wasmtime".
fn bench_plugin_surfaces_at_count(count: usize) {
    let runtime = PluginRuntime::shared().expect("engine available on the bench host");
    let compat = HostCompatibility::v1("0.3.0");

    let installer_dir = scratch_dir(&format!("installed-{count}"));
    install_fixture_copies(
        &installer_dir,
        count,
        vec![PluginExtension::Runner],
        Components::Identical,
    );
    let installer_registry = PluginRegistry::new(installer_dir.clone(), compat.clone());
    for _ in 0..WARMUP_ITERATIONS {
        let _ = installer_registry.installed_plugins(&runtime);
    }
    let mut installed_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| installer_registry.installed_plugins(&runtime));
        installed_samples.push(elapsed);
    }
    report(
        "plugin_installer.get_plugin_catalog",
        count,
        installed_samples,
    );
    fs::remove_dir_all(&installer_dir).ok();

    let runner_dir = scratch_dir(&format!("runner-{count}"));
    install_fixture_copies(
        &runner_dir,
        count,
        vec![PluginExtension::Runner],
        Components::Identical,
    );
    let runner_registry = PluginRegistry::new(runner_dir.clone(), compat);
    for _ in 0..WARMUP_ITERATIONS {
        let _ = runner_registry.runner_plugins(&runtime);
    }
    let mut runner_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| runner_registry.runner_plugins(&runtime));
        runner_samples.push(elapsed);
    }
    report("lib.get_runner_plugins", count, runner_samples);
    fs::remove_dir_all(&runner_dir).ok();
}

#[test]
#[ignore]
fn bench_plugin_surfaces_0_installed() {
    bench_plugin_surfaces_at_count(0);
}

#[test]
#[ignore]
fn bench_plugin_surfaces_1_installed() {
    bench_plugin_surfaces_at_count(1);
}

#[test]
#[ignore]
fn bench_plugin_surfaces_8_installed() {
    bench_plugin_surfaces_at_count(8);
}

/// 20 crosses `MAX_PROBED_PLUGINS` (16 in `plugin_registry.rs`): the point of
/// this run is to show the cost stops growing past the bound, not to pretend
/// 20 plugins is a realistic library.
#[test]
#[ignore]
fn bench_plugin_surfaces_20_installed() {
    bench_plugin_surfaces_at_count(20);
}

// ---------------------------------------------------------------------------
// P5 — what the compile cache removes from the two discovery commands
// ---------------------------------------------------------------------------

/// A fixed key, because the bench must not touch the keychain: a `cargo test`
/// binary asking macOS for a keychain item is a password prompt on a machine
/// nobody is watching. What is measured here is the disk and the engine, and
/// neither depends on where the key came from.
const CACHE_BENCH_KEY: [u8; 32] = [0x5a; 32];

/// Three numbers for the same work: no cache at all (the state before this lot),
/// a cache seeing each component for the first time, and a cache that already
/// holds them.
///
/// The uncached line is measured here rather than quoted from section 3 of
/// `docs/performance.md` so that the comparison survives a different machine, a
/// different day and — because these copies carry distinct component bytes,
/// unlike the ones those numbers were taken with — a different fixture shape.
fn bench_plugin_compile_cache_at_count(count: usize) {
    let root = scratch_dir(&format!("cache-plugins-{count}"));
    install_fixture_copies(
        &root,
        count,
        vec![PluginExtension::Runner],
        Components::Distinct,
    );
    let registry = PluginRegistry::new(root.clone(), HostCompatibility::v1("0.3.0"));

    let uncached = PluginRuntime::new().expect("an engine is available on the bench host");
    for _ in 0..WARMUP_ITERATIONS {
        let _ = registry.installed_plugins(&uncached);
    }
    let mut samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| registry.installed_plugins(&uncached));
        samples.push(elapsed);
    }
    report("plugin discovery, no cache", count, samples);

    let artifacts = scratch_dir(&format!("cache-artifacts-{count}"));
    let cached = PluginRuntime::new().expect("an engine is available on the bench host");
    cached.use_compile_cache(ComponentCache::open(
        cached.engine().clone(),
        artifacts.clone(),
        CACHE_BENCH_KEY,
        CacheLimits::default(),
    ));

    // The first pass compiles *and* writes, so it is strictly slower than the
    // uncached one. That cost is paid once per component, ever, and saying so is
    // the point of measuring it separately rather than folding it into a median.
    let (_, cold) = timed(|| registry.installed_plugins(&cached));
    report("plugin discovery, cold cache", count, vec![cold]);

    let mut warm = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| registry.installed_plugins(&cached));
        warm.push(elapsed);
    }
    report("plugin discovery, warm cache", count, warm);
    println!(
        "PERF cache counts                        n={count:<6} {:?}",
        cached.compile_cache_counts()
    );

    fs::remove_dir_all(&artifacts).ok();
    fs::remove_dir_all(&root).ok();
}

// P6 — the official Ryujinx runner, installed
// ---------------------------------------------------------------------------
//
// Step 2.4 of `docs/plugin-system-plan.md` asks for startup, navigation, import
// and first launch measured *with a plugin installed*, and until this lot there
// was no plugin to install: the benches above multiply the adversarial fixture
// by a count, which answers "what does discovery cost per component" and nothing
// about a real library. These four answer the rest of the question with the
// shipped `plugins/ryujinx` component, a fabricated `.app` bundle and folders of
// fake ROM files — never a real dump, a real key or a real emulator.
//
// Navigation and the rail are deliberately absent here: no plugin runs on those
// paths at all, which is an architectural fact rather than a measurement, and
// `docs/performance.md`'s section 1 measures them in the browser where they
// actually live.

/// Filenames a dumped Switch library really uses, so the import pays for what a
/// user's folder costs: a name the opaque-id grammar cannot spell, resolved by
/// the host through the hex reference.
fn synthetic_switch_library(directory: &Path, count: usize) -> Vec<String> {
    let mut names = Vec::with_capacity(count);
    for index in 0..count {
        let name = format!("Bench Game {index} [0100000000{index:06X}][v0].nsp");
        fs::write(directory.join(&name), b"not a Nintendo Switch dump").unwrap();
        names.push(name);
    }
    names
}

/// A service over a freshly installed Ryujinx package, with one accepted profile
/// and one granted folder — the state the "Add an emulator" flow leaves behind.
fn ryujinx_service(
    root: &Path,
    games: &Path,
    profile_id: &str,
    artifacts: Option<&Path>,
) -> std::sync::Arc<crate::runner_commands::ThirdPartyRunnerService> {
    use crate::ryujinx_plugin::{
        GAMES_SLOT, PLUGIN_ID, fake_application, manifest, package_archive,
    };

    let plugin_root = root.join("plugins");
    let installer = crate::plugin_installer::PluginInstallerService::new(
        plugin_root.clone(),
        env!("CARGO_PKG_VERSION"),
    );
    let files = crate::plugin_installer::read_package(&package_archive()).unwrap();
    crate::plugin_installer::install_verified(
        &installer,
        PLUGIN_ID,
        manifest().version.as_str(),
        crate::plugin_update::PackageChannel::Development,
        &files,
        // A hand-loaded package on the development channel, which is the door
        // with no downgrade rule to re-check: `expected` is only `Some` through
        // the registry.
        false,
    )
    .expect("the shipped package must install");

    let store = crate::runner_host::CatalogStore::new(
        std::sync::Arc::new(std::sync::RwLock::new(Catalog::default())),
        root.join("catalog.json"),
        std::sync::Arc::new(std::sync::Mutex::new(())),
    );
    let mut service = crate::runner_commands::ThirdPartyRunnerService::new(
        store,
        plugin_root,
        HostCompatibility::v1(env!("CARGO_PKG_VERSION")),
    );
    // The service's own engine, not `PluginRuntime::shared()`: the process-wide
    // one has no cache directory configured in a test binary, so it is the
    // cacheless state whatever P5 does, and a bench that wants to show the cache
    // has to bring one. `CACHE_BENCH_KEY` is a fixed key for the reason that
    // constant gives — nothing here may reach the keychain.
    if let Some(artifacts) = artifacts {
        let runtime = PluginRuntime::new().expect("an engine is available on the bench host");
        runtime.use_compile_cache(ComponentCache::open(
            runtime.engine().clone(),
            artifacts.to_path_buf(),
            CACHE_BENCH_KEY,
            CacheLimits::default(),
        ));
        service = service.with_runtime(runtime);
    }
    let service = std::sync::Arc::new(service);
    service
        .create_profile_with_id(profile_id, PLUGIN_ID, "Ryujinx", &fake_application(root))
        .unwrap();
    service
        .grant_directory(profile_id, Some(GAMES_SLOT), games)
        .unwrap();
    service
}

/// Import a library of `count` fake ROMs, then the first launch it makes
/// possible, then what those cards cost the next startup.
///
/// Three numbers, because they are three different questions. The first import
/// includes one cold Wasmtime compile of the component and `count` catalog
/// transactions; a second import over the same folder refreshes instead of
/// inserting, which is the cost of the "check for new games" a user will press;
/// and `load_with_migration` afterwards is the only part of any of this that
/// startup pays for.
fn bench_ryujinx_import_at_size(count: usize) {
    let root = scratch_dir(&format!("ryujinx-{count}"));
    let games = root.join("games");
    fs::create_dir_all(&games).unwrap();
    let names = synthetic_switch_library(&games, count);
    let profile_id = "runner-ryujinx-bench";
    let service = ryujinx_service(&root, &games, profile_id, None);

    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (first, first_elapsed) =
        timed(|| service.import_now(profile_id, &cancelled, |_| {}).unwrap());
    report(
        "ryujinx.import (no cache, first)",
        count,
        vec![first_elapsed],
    );
    println!(
        "PERF {:<40} n={count:<6} imported={} skipped={} pages={}",
        "ryujinx.import outcome",
        first.progress.imported,
        first.progress.skipped,
        first.progress.pages
    );

    let mut again = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| service.import_now(profile_id, &cancelled, |_| {}).unwrap());
        again.push(elapsed);
    }
    report("ryujinx.import (no cache, again)", count, again);

    // The same two numbers with P5's compile cache (#49) behind them, over a
    // second root so the first import is an insert in both runs.
    //
    // Both of these are cache *hits*, and deliberately so: `ryujinx_service`
    // creates the profile before returning, which loads the package and therefore
    // compiles the component once. That is not an artefact of the bench, it is
    // production — `ThirdPartyRunnerService::runtime` permits the cache on every
    // runner gesture, and the gesture that pays the cold compile is the one that
    // picked the emulator, not the import. The cold cost itself is measured where
    // it is actually paid, by `bench_ryujinx_plugin_surfaces` below.
    let cached_root = scratch_dir(&format!("ryujinx-cached-{count}"));
    let cached_games = cached_root.join("games");
    fs::create_dir_all(&cached_games).unwrap();
    synthetic_switch_library(&cached_games, count);
    let artifacts = scratch_dir(&format!("ryujinx-artifacts-{count}"));
    let cached = ryujinx_service(&cached_root, &cached_games, profile_id, Some(&artifacts));
    let (_, hit) = timed(|| cached.import_now(profile_id, &cancelled, |_| {}).unwrap());
    report("ryujinx.import (cache hit, first)", count, vec![hit]);
    let mut warm = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| cached.import_now(profile_id, &cancelled, |_| {}).unwrap());
        warm.push(elapsed);
    }
    report("ryujinx.import (cache hit, again)", count, warm);
    fs::remove_dir_all(&cached_root).ok();
    fs::remove_dir_all(&artifacts).ok();

    // The first launch: `prepare-launch` under the interactive budget, the
    // intent validated, then the host resolving a bundle executable and a game
    // file. Everything except starting the process, which would start a real
    // emulator on a real machine.
    let package = service.package(crate::ryujinx_plugin::PLUGIN_ID).unwrap();
    let catalog = Catalog::load(&root.join("catalog.json")).unwrap();
    if let Some(name) = names.first() {
        let game_ref = crate::ryujinx_plugin::reference(name);
        let mut launches = Vec::with_capacity(MEASURED_ITERATIONS);
        for _ in 0..MEASURED_ITERATIONS {
            let (prepared, elapsed) = timed(|| {
                crate::runner_host::prepare_runner_launch(
                    &package, &catalog, profile_id, &game_ref, &cancelled,
                )
            });
            prepared.expect("the first game must be launchable");
            launches.push(elapsed);
        }
        report("ryujinx.prepare_launch", count, launches);
    }

    let catalog_path = root.join("catalog.json");
    let mut loads = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| Catalog::load_with_migration(&catalog_path).unwrap());
        loads.push(elapsed);
    }
    report("catalog.load_with_migration (runner cards)", count, loads);

    fs::remove_dir_all(&root).ok();
}

#[test]
#[ignore]
fn bench_plugin_compile_cache_1_installed() {
    bench_plugin_compile_cache_at_count(1);
}

#[test]
#[ignore]
fn bench_plugin_compile_cache_8_installed() {
    bench_plugin_compile_cache_at_count(8);
}

#[test]
#[ignore]
fn bench_plugin_compile_cache_20_installed() {
    bench_plugin_compile_cache_at_count(20);
}

#[test]
#[ignore]
fn bench_ryujinx_import_1_rom() {
    bench_ryujinx_import_at_size(1);
}

#[test]
#[ignore]
fn bench_ryujinx_import_100_roms() {
    bench_ryujinx_import_at_size(100);
}

/// 1000 crosses `MAX_DIRECTORY_ENTRIES` (256 in `plugin_runtime.rs`): a granted
/// folder is listed no further than that, so this run shows where a large
/// library stops being visible to *any* plugin rather than pretending 1000 files
/// import. `docs/performance.md` records what comes back.
#[test]
#[ignore]
fn bench_ryujinx_import_1000_roms() {
    bench_ryujinx_import_at_size(1_000);
}

/// Settings → Plugins and the emulator flow, with the real Ryujinx package
/// installed instead of N copies of the fixture. Same two commands as
/// `bench_plugin_surfaces_at_count`; what changes is that this is the component
/// a user will actually have, so the per-component compile cost above can be
/// checked against a second, independent component rather than assumed.
#[test]
#[ignore]
fn bench_ryujinx_plugin_surfaces() {
    use crate::ryujinx_plugin::{COMPONENT, MANIFEST_JSON, PLUGIN_ID};

    let runtime = PluginRuntime::shared().expect("engine available on the bench host");
    let dir = scratch_dir("ryujinx-surfaces");
    let installed = dir.join(PLUGIN_ID);
    fs::create_dir_all(&installed).unwrap();
    fs::write(installed.join("component.wasm"), COMPONENT).unwrap();
    fs::write(installed.join("manifest.json"), MANIFEST_JSON).unwrap();
    let registry = PluginRegistry::new(dir.clone(), HostCompatibility::v1("0.3.0"));

    for _ in 0..WARMUP_ITERATIONS {
        let _ = registry.installed_plugins(&runtime);
    }
    let mut installed_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut runner_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| registry.installed_plugins(&runtime));
        installed_samples.push(elapsed);
        let (_, elapsed) = timed(|| registry.runner_plugins(&runtime));
        runner_samples.push(elapsed);
    }
    report(
        "ryujinx.get_plugin_catalog (no cache)",
        1,
        installed_samples,
    );
    report("ryujinx.get_runner_plugins (no cache)", 1, runner_samples);

    // And with P5's cache, which is what both commands really get in production:
    // they are two of `plugin_compile_cache::permit`'s call sites.
    let artifacts = scratch_dir("ryujinx-surface-artifacts");
    let cached = PluginRuntime::new().expect("an engine is available on the bench host");
    cached.use_compile_cache(ComponentCache::open(
        cached.engine().clone(),
        artifacts.clone(),
        CACHE_BENCH_KEY,
        CacheLimits::default(),
    ));
    let (_, cold) = timed(|| registry.installed_plugins(&cached));
    report("ryujinx.get_plugin_catalog (cold cache)", 1, vec![cold]);
    let mut warm = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| registry.installed_plugins(&cached));
        warm.push(elapsed);
    }
    report("ryujinx.get_plugin_catalog (warm cache)", 1, warm);
    println!(
        "PERF ryujinx cache counts                n=1      {:?}",
        cached.compile_cache_counts()
    );

    fs::remove_dir_all(&artifacts).ok();
    fs::remove_dir_all(&dir).ok();
}
