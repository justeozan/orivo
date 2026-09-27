//! The shipped Ryujinx runner, through the developer kit an outside author
//! would use on it.
//!
//! `src-tauri/src/ryujinx_plugin.rs` already proves the plugin works on Orivo's
//! own path — install, profile, grant, import, process. This file asks a
//! narrower question the other cannot: does the *published* toolchain accept the
//! package we publish? A validator or a simulator that the first-party plugin
//! would fail is a tool nobody else should be handed either.
//!
//! The package lives in the repository as `manifest.json` + `component.wasm` and
//! deliberately carries no `signature.ed25519`: signing is the registry's, and
//! who holds its key is an open question (`docs/ryujinx-runner.md`). So the
//! package rules are checked against a temporary copy with a development-channel
//! signature beside it, which is exactly the state a hand-loaded package is in —
//! while the manifest itself is validated in place, from the committed file.

use orivo_plugin_sdk::manifest_check::{validate_manifest_file, validate_package_dir};
use orivo_plugin_sdk::simulate::{self, SimulatedGrant, SimulatedRequest};
use std::{fs, path::Path, path::PathBuf, time::Duration};

const COMPONENT: &[u8] = include_bytes!("../../../plugins/ryujinx/package/component.wasm");
const MANIFEST_JSON: &str = include_str!("../../../plugins/ryujinx/package/manifest.json");
const PLUGIN_ID: &str = "com.orivo.ryujinx";
/// The grant slot the component hard-codes; the v1 manifest has no field where a
/// package could declare it, so an author has to read it out of the source.
const GAMES_GRANT: &str = "games";
const PROFILE_ID: &str = "runner-ryujinx-1";

const SHIPPED_MANIFEST: &str = "../../plugins/ryujinx/package/manifest.json";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orivo-ryujinx-sdk-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The shipped package, plus the one file the repository does not carry.
fn package_copy(tag: &str) -> PathBuf {
    let dir = scratch(tag);
    fs::write(dir.join("manifest.json"), MANIFEST_JSON).unwrap();
    fs::write(dir.join("component.wasm"), COMPONENT).unwrap();
    fs::write(dir.join("signature.ed25519"), b"development").unwrap();
    dir
}

/// A folder named the way a dumped library is, so what the simulator prints is
/// what an author would see against their own disk.
fn library(tag: &str) -> PathBuf {
    let dir = scratch(tag);
    for name in [
        "Super Mario Odyssey [0100000000010000][v0].nsp",
        "Celeste (USA).nsp",
        "prod.keys",
    ] {
        fs::write(dir.join(name), b"not a Nintendo Switch dump").unwrap();
    }
    dir
}

fn reference(name: &str) -> String {
    name.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The committed manifest, validated where it lives. A relative path is enough:
/// `cargo test` runs an integration test with the crate root as its working
/// directory.
#[test]
fn the_committed_manifest_passes_the_validator_in_place() {
    let report = validate_manifest_file(Path::new(SHIPPED_MANIFEST))
        .unwrap_or_else(|errors| panic!("{errors:?}"));
    assert_eq!(report.manifest.id(), PLUGIN_ID);
}

#[test]
fn the_shipped_package_validates_as_a_development_channel_package() {
    let dir = package_copy("validate");
    let report = validate_package_dir(&dir).unwrap_or_else(|errors| panic!("{errors:?}"));
    assert_eq!(report.manifest.id(), PLUGIN_ID);
    assert!(
        report.hash_mismatches.is_empty(),
        "run plugins/ryujinx/build.sh and update the manifest: {:?}",
        report.hash_mismatches
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The registry's own pre-install gate: the component's type may ask for no more
/// than the manifest declares, and `get-identity`/`health-check` have to answer
/// under the probe budget with no grants at all.
#[test]
fn the_contract_check_the_registry_runs_accepts_it() {
    let dir = package_copy("check");
    let report = simulate::check_contract(&dir).unwrap_or_else(|errors| panic!("{errors:?}"));
    assert!(
        report
            .contract
            .required_capabilities
            .contains(&orivo_lib::plugin_manifest::PluginCapability::FilesRead)
    );
    let health = report
        .health
        .unwrap_or_else(|error| panic!("verify_runner refused the shipped package: {error}"))
        .expect("ContractAndHealth must run the health check");
    assert!(health.ready);
    let _ = fs::remove_dir_all(&dir);
}

/// One page through the real `PluginRuntime`, under the same fuel, deadline and
/// memory ceilings Orivo enforces. Two games out of three files, with the keys
/// left alone.
#[test]
fn the_simulator_discovers_a_page_from_a_granted_folder() {
    let dir = package_copy("discover");
    let games = library("discover-games");
    let report = simulate::simulate(
        &dir,
        &[SimulatedGrant {
            name: GAMES_GRANT.into(),
            path: games.clone(),
        }],
        SimulatedRequest::DiscoverPage {
            profile_id: PROFILE_ID.into(),
            cursor: None,
            limit: 10,
        },
        Duration::from_secs(10),
    )
    .unwrap_or_else(|errors| panic!("{errors:?}"));

    match report.outcome {
        Ok(orivo_lib::plugin_runtime::PluginResponse::DiscoveryPage(page)) => {
            let mut titles = page
                .games
                .iter()
                .map(|game| game.title.as_str())
                .collect::<Vec<_>>();
            titles.sort();
            assert_eq!(titles, vec!["Celeste (USA)", "Super Mario Odyssey"]);
            assert!(page.complete);
        }
        other => panic!("expected a discovery page, got {other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&games);
}

/// A prepared launch is opaque ids and a declared mode. The simulator is where
/// an author sees that, without a catalogue or a profile behind it.
#[test]
fn the_simulator_prepares_a_launch_intent_of_opaque_ids_only() {
    let dir = package_copy("prepare");
    let report = simulate::simulate(
        &dir,
        &[],
        SimulatedRequest::PrepareLaunch {
            profile_id: PROFILE_ID.into(),
            game_reference: reference("Celeste (USA).nsp"),
        },
        Duration::from_secs(10),
    )
    .unwrap_or_else(|errors| panic!("{errors:?}"));

    match report.outcome {
        Ok(orivo_lib::plugin_runtime::PluginResponse::LaunchIntent(intent)) => {
            assert_eq!(intent.runner_id(), PLUGIN_ID);
            assert_eq!(intent.profile_id(), PROFILE_ID);
            assert_eq!(intent.game_reference(), reference("Celeste (USA).nsp"));
        }
        other => panic!("expected a launch intent, got {other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
}
