//! End-to-end proof that the simulator calls the real host rather than a
//! stand-in: it packages the same third-party component Orivo's own sandbox
//! suite is written against — `src-tauri/fixtures/orivo-runner-fixture.wasm`
//! — and runs it through `orivo_plugin_sdk::simulate` exactly as an author
//! would point the CLI at their own package directory.
//!
//! If this ever needs a different fixture, keep it the committed one:
//! `src-tauri/fixtures/README.md` is the single source of truth for what each
//! `fixture:*` selector does, and duplicating that table here would be the
//! kind of second copy the rest of this SDK avoids.

use orivo_plugin_sdk::simulate::{self, SimulatedGrant, SimulatedRequest};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};

const FIXTURE_WASM: &[u8] =
    include_bytes!("../../../src-tauri/fixtures/orivo-runner-fixture.wasm");

fn package_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orivo-plugin-sdk-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();

    let mut digest = Sha256::new();
    digest.update(FIXTURE_WASM);
    let sha256 = format!("{:x}", digest.finalize());

    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
                "id": "com.orivo.fixture-runner",
                "name": "Fixture Runner",
                "version": "1.0.0",
                "sdk": "orivo-plugin@1",
                "extensions": ["runner"],
                "capabilities": ["runner_prepare", "files_read"],
                "artifacts": [
                    {{"path": "component.wasm", "kind": "component", "sha256": "{sha256}", "byteSize": {len}}}
                ]
            }}"#,
            len = FIXTURE_WASM.len()
        ),
    )
    .unwrap();
    fs::write(dir.join("component.wasm"), FIXTURE_WASM).unwrap();
    fs::write(dir.join("signature.ed25519"), b"development").unwrap();
    dir
}

fn games_grant() -> (tempfile_dir::TempDir, SimulatedGrant) {
    let holder = tempfile_dir::TempDir::new("orivo-plugin-sdk-e2e-games");
    fs::write(holder.path().join("alpha.rom"), b"Alpha Quest\n").unwrap();
    let grant = SimulatedGrant {
        name: "fixture-games".into(),
        path: holder.path().to_path_buf(),
    };
    (holder, grant)
}

/// A tiny stand-in for a temp-dir crate: this SDK has no need for one anywhere
/// else, so pulling in a dependency purely to get `Drop`-based cleanup in a
/// test did not seem worth it next to five lines.
mod tempfile_dir {
    use std::{fs, path::PathBuf};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn checks_the_contract_and_health_of_the_real_fixture() {
    let dir = package_dir("check");
    let report = simulate::check_contract(&dir).expect("the fixture's contract must be readable");
    // The fixture imports `host-files` to read its granted games folder, so
    // the registry's own contract check must see that requirement too.
    assert!(
        report
            .contract
            .required_capabilities
            .contains(&orivo_lib::plugin_manifest::PluginCapability::FilesRead)
    );
    let health = report.health.expect("verify_runner must accept the fixture");
    let health = health.expect("ContractAndHealth must run the health check");
    assert!(health.ready);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn prepares_a_launch_intent_through_the_real_host() {
    let dir = package_dir("prepare-launch");
    let report = simulate::simulate(
        &dir,
        &[],
        SimulatedRequest::PrepareLaunch {
            profile_id: "fixture-profile-1".into(),
            game_reference: "fixture:ok".into(),
        },
        Duration::from_secs(5),
    )
    .expect("the request must reach the host");

    match report.outcome {
        Ok(orivo_lib::plugin_runtime::PluginResponse::LaunchIntent(intent)) => {
            assert_eq!(intent.game_reference(), "fixture:ok");
        }
        other => panic!("expected a launch intent, got {other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn refuses_a_directory_grant_the_manifest_never_named() {
    let dir = package_dir("deny");
    let (_holder, grant) = games_grant();
    let report = simulate::simulate(
        &dir,
        &[grant],
        SimulatedRequest::PrepareLaunch {
            profile_id: "fixture-profile-1".into(),
            game_reference: "fixture:deny".into(),
        },
        Duration::from_secs(5),
    )
    .expect("the request must reach the host");

    // `fixture:deny` asks for a grant ("fixture-other") it was never given.
    // The host's refusal comes back as a typed plugin error inside a
    // successful call, not as a transport failure — that is the whole point
    // of `permission-denied` being part of the WIT result type.
    match report.outcome {
        Err(message) => assert!(message.to_lowercase().contains("not allowed")),
        Ok(response) => panic!("expected a permission refusal, got {response:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discovers_a_page_of_games_from_a_simulated_grant() {
    let dir = package_dir("discover");
    let (_holder, grant) = games_grant();
    let report = simulate::simulate(
        &dir,
        &[grant],
        SimulatedRequest::DiscoverPage {
            profile_id: "fixture-profile-1".into(),
            cursor: None,
            limit: 10,
        },
        Duration::from_secs(5),
    )
    .expect("the request must reach the host");

    match report.outcome {
        Ok(orivo_lib::plugin_runtime::PluginResponse::DiscoveryPage(page)) => {
            assert_eq!(page.games.len(), 1);
            assert_eq!(page.games[0].title, "Alpha Quest");
        }
        other => panic!("expected a discovery page, got {other:?}"),
    }
    assert!(
        !report.decisions.is_empty() || !report.plugin_messages.is_empty(),
        "the journal should have something to say about a successful discovery"
    );
    let _ = fs::remove_dir_all(&dir);
}
