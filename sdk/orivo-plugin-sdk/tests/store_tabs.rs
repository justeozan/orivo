//! A Store tab plugin through the author's loop: `check` holds it to the
//! `store-tab-plugin` world, and `simulate --request store-tabs` prints what the
//! Store would receive — validated by the real host, not by a copy of its rules.
//!
//! The component is the committed `src-tauri/fixtures/orivo-store-tab-fixture.wasm`;
//! `src-tauri/fixtures/store-tab-fixture/src/lib.rs` lists the faults its one
//! answer carries on purpose.

use orivo_lib::plugin_runtime::PluginResponse;
use orivo_plugin_sdk::simulate::{self, SimulatedRequest};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};

const STORE_FIXTURE: &[u8] =
    include_bytes!("../../../src-tauri/fixtures/orivo-store-tab-fixture.wasm");
const RUNNER_FIXTURE: &[u8] =
    include_bytes!("../../../src-tauri/fixtures/orivo-runner-fixture.wasm");

fn package_dir(tag: &str, component: &[u8], extensions: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orivo-plugin-sdk-store-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let mut digest = Sha256::new();
    digest.update(component);
    let sha256 = format!("{:x}", digest.finalize());
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
                "id": "com.orivo.fixture-store",
                "name": "Fixture Store",
                "version": "1.0.0",
                "sdk": "orivo-plugin@1",
                "extensions": [{extensions}],
                "capabilities": [],
                "artifacts": [
                    {{"path": "component.wasm", "kind": "component", "sha256": "{sha256}", "byteSize": {len}}}
                ]
            }}"#,
            len = component.len()
        ),
    )
    .unwrap();
    fs::write(dir.join("component.wasm"), component).unwrap();
    dir
}

#[test]
fn checks_a_store_tab_package_against_its_own_world() {
    let dir = package_dir("check", STORE_FIXTURE, r#""store_tab""#);
    let report = simulate::check_contract(&dir).expect("the fixture keeps the Store tab contract");
    assert!(report.contract.required_capabilities.is_empty());
    assert!(matches!(report.health, Ok(None)));
    let _ = fs::remove_dir_all(&dir);

    // The runner fixture compiles and keeps a contract — not this one.
    let dir = package_dir("check-runner", RUNNER_FIXTURE, r#""store_tab""#);
    let errors = simulate::check_contract(&dir).unwrap_err();
    assert!(
        errors[0].message.contains("store tab contract"),
        "{errors:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn simulates_what_the_store_would_receive() {
    let dir = package_dir("simulate", STORE_FIXTURE, r#""store_tab""#);
    let report = simulate::simulate(
        &dir,
        &[],
        SimulatedRequest::StoreTabs,
        Duration::from_secs(5),
    )
    .expect("the request must reach the host");
    let Ok(PluginResponse::StoreTabs(tabs)) = report.outcome else {
        panic!("expected store tabs, got {:?}", report.outcome);
    };
    assert_eq!(tabs.len(), 2, "the tab with no games is not shown");
    assert_eq!(tabs[0].accent.as_deref(), Some("#e4572e"));
    assert_eq!(tabs[0].games[0].id, "com.orivo.fixture-store:alpha");
    assert_eq!(tabs[0].games[1].page_url, None, "javascript: was dropped");
    assert!(
        report
            .plugin_messages
            .iter()
            .any(|entry| entry.detail.contains("three tabs")),
        "the plugin's own log line comes back with the answer"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Orivo never asks a package for tabs its manifest did not declare, so the
/// simulator does not either.
#[test]
fn refuses_to_simulate_a_store_tab_request_the_manifest_never_declared() {
    let dir = package_dir("undeclared", STORE_FIXTURE, r#""ui_contribution""#);
    let errors = simulate::simulate(
        &dir,
        &[],
        SimulatedRequest::StoreTabs,
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(errors[0].message.contains("StoreTab"), "{errors:?}");
    let _ = fs::remove_dir_all(&dir);
}
