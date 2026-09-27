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
//! 2. Are the two adoption passes (Wine, Winlator) cheap no-ops when nothing
//!    needs them, and still linear rather than quadratic when something does?
//!    Only the Wine one is on the startup path: since M1 the Winlator pass runs
//!    in the background after the first paint, and is measured here for its own
//!    sake rather than as a startup cost.
//! 3. What does having plugins installed cost the two on-demand commands that
//!    actually touch the plugin runtime — `get_plugin_catalog` (Settings ›
//!    Plugins) and `get_runner_plugins` (the emulator flow) — neither of which
//!    sits on the startup, navigation, rail or search path? `plugin_runtime.rs`
//!    already has `reports_what_a_prepared_component_costs_against_a_cold_one`
//!    for the cost of a single cold component; this module multiplies that by
//!    plugin count instead of re-measuring it.

use crate::catalog::{CURRENT_SCHEMA_VERSION, Catalog};
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
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
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
    println!(
        "PERF {label:<40} n={n:<6} min={min:>10.2?} median={median:>10.2?} max={max:>10.2?}"
    );
}

fn timed<T>(mut work: impl FnMut() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = work();
    (value, start.elapsed())
}

/// A `catalog.json` body with `count` games, a fraction of them local `.exe`
/// Direct games so `auto_apply_wine_to_direct_games`'s filter has something to
/// walk instead of short-circuiting on an empty catalog. Built as text, not as
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

/// `load_with_migration` + `save_atomically` + the Wine auto-apply pass, at one
/// catalog size: the sequence `AppState::load` runs on every startup. The
/// Winlator adoption pass is measured alongside them because its number is worth
/// having, but it is no longer part of that sequence — it runs on a background
/// worker once the shell is on screen.
fn bench_catalog_at_size(n: usize) {
    let dir = scratch_dir(&format!("catalog-{n}"));
    let catalog_path = dir.join("catalog.json");
    fs::write(&catalog_path, synthetic_catalog_json(n)).unwrap();
    let wine_prefix_root = dir.join("wine-prefixes");

    for _ in 0..WARMUP_ITERATIONS {
        let _ = Catalog::load_with_migration(&catalog_path).unwrap();
    }

    let mut load_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut save_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut wine_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut winlator_samples = Vec::with_capacity(MEASURED_ITERATIONS);

    for _ in 0..MEASURED_ITERATIONS {
        let (loaded, load_elapsed) =
            timed(|| Catalog::load_with_migration(&catalog_path).unwrap());
        load_samples.push(load_elapsed);

        let mut catalog = loaded.catalog;
        let save_path = dir.join("catalog.save.json");
        let (_, save_elapsed) = timed(|| catalog.save_atomically(&save_path).unwrap());
        save_samples.push(save_elapsed);

        // Both passes stop early where they have nothing to do — Wine behind
        // its `#[cfg]`, Winlator on a folder that is not there — so on any
        // other platform, including this bench run most of the time, they
        // return `false` after their `O(n)` filter without touching Wine or
        // Winlator at all. That early return is still measured here, not
        // assumed, because it is exactly what a startup actually pays.
        let (_, wine_elapsed) =
            timed(|| crate::auto_apply_wine_to_direct_games(&mut catalog, &wine_prefix_root));
        wine_samples.push(wine_elapsed);

        // The folder resolution is part of what the background pass pays, so it
        // is inside the timed closure rather than hoisted out of it.
        let cancelled = AtomicBool::new(false);
        let (_, winlator_elapsed) = timed(|| {
            let folder = crate::winlator_export_folder(&catalog);
            crate::adopt_exported_winlator_shortcuts(&mut catalog, &folder, &cancelled)
        });
        winlator_samples.push(winlator_elapsed);
    }

    report("catalog.load_with_migration", n, load_samples);
    report("catalog.save_atomically", n, save_samples);
    report("auto_apply_wine_to_direct_games", n, wine_samples);
    report("adopt_exported_winlator_shortcuts", n, winlator_samples);

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
fn install_fixture_copies(root: &Path, count: usize, extensions: Vec<PluginExtension>) {
    for index in 0..count {
        let id = if index == 0 {
            RUNNER_ID.to_string()
        } else {
            format!("com.orivo.bench-runner-{index}")
        };
        let directory = root.join(&id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("component.wasm"), RUNNER_COMPONENT).unwrap();
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
                sha256: sha256_hex(RUNNER_COMPONENT),
                byte_size: RUNNER_COMPONENT.len() as u64,
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
    install_fixture_copies(&installer_dir, count, vec![PluginExtension::Runner]);
    let installer_registry = PluginRegistry::new(installer_dir.clone(), compat.clone());
    for _ in 0..WARMUP_ITERATIONS {
        let _ = installer_registry.installed_plugins(&runtime);
    }
    let mut installed_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for _ in 0..MEASURED_ITERATIONS {
        let (_, elapsed) = timed(|| installer_registry.installed_plugins(&runtime));
        installed_samples.push(elapsed);
    }
    report("plugin_installer.get_plugin_catalog", count, installed_samples);
    fs::remove_dir_all(&installer_dir).ok();

    let runner_dir = scratch_dir(&format!("runner-{count}"));
    install_fixture_copies(&runner_dir, count, vec![PluginExtension::Runner]);
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
