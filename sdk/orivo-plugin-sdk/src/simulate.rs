//! Runs a packaged runner plugin against the real host: the same
//! `PluginRuntime`, the same fuel/deadline/memory ceilings, the same grant
//! resolution Orivo uses. Nothing here reimplements a seam — every call below
//! is one `orivo_lib::plugin_runtime::PluginRuntime` already exposes publicly.
//!
//! Only `runner-plugin` is invocable, because it is the only world Orivo's
//! host calls today (`wit/README.md`). `source-plugin`, `metadata-plugin` and
//! `ui-plugin` stay contract-only until a corresponding host path exists —
//! simulating them here would be simulating something Orivo cannot run yet.

use crate::manifest_check::{LocatedError, validate_manifest_file};
use orivo_lib::plugin_manifest::{CapabilityGrant, CapabilityScope, PluginCapability, PluginExtension};
use orivo_lib::plugin_runtime::{
    ComponentContract, JournalEntry, PluginGrants, PluginRequest, PluginResponse, PluginRuntime,
    RunnerCheck,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

/// A directory grant an author hands to the simulated plugin, the same shape
/// `PluginGrants::resolve` expects from a real persisted grant: an opaque name
/// the component will ask for, mapped to a real folder only the host knows.
#[derive(Debug, Clone)]
pub struct SimulatedGrant {
    pub name: String,
    pub path: PathBuf,
}

/// What to ask the component. Mirrors `PluginRequest` one-to-one; kept
/// separate so the CLI layer does not have to depend on a request's exact
/// grant/budget wiring to build one.
#[derive(Debug, Clone)]
pub enum SimulatedRequest {
    Identity,
    HealthCheck,
    ValidateProfile { profile_id: String, display_name: String },
    DiscoverPage { profile_id: String, cursor: Option<String>, limit: u32 },
    PrepareLaunch { profile_id: String, game_reference: String },
}

/// A finished run: the call's own answer, plus the host's account of what it
/// decided along the way. Journal entries come back even on failure — that is
/// usually the more useful half of the report, since it says *why* a refusal
/// happened rather than only that one did.
#[derive(Debug)]
pub struct SimulationReport {
    pub outcome: Result<PluginResponse, String>,
    pub decisions: Vec<JournalEntry>,
    pub plugin_messages: Vec<JournalEntry>,
}

/// The registry's own pre-install check: does the component's type agree with
/// its manifest, and does it answer `get-identity`/`health-check` honestly?
/// This runs under the probe budget with no grants at all, exactly as Orivo
/// runs it before ever offering a runner for configuration.
#[derive(Debug)]
pub struct ContractCheckReport {
    pub contract: ComponentContract,
    pub health: Result<Option<orivo_lib::plugin_runtime::PluginHealth>, String>,
}

pub fn check_contract(dir: &Path) -> Result<ContractCheckReport, Vec<LocatedError>> {
    let manifest = validate_manifest_file(&dir.join("manifest.json"))?.manifest;
    let bytes = read_component(dir, &manifest)?;
    let runtime = PluginRuntime::new().map_err(|error| single(dir, error.to_string()))?;
    let sha256 = sha256_hex(&bytes);
    let prepared = runtime
        .prepare_component(&bytes, &sha256)
        .map_err(|error| single(dir, error.to_string()))?;

    let contract = runtime
        .inspect_contract(&prepared)
        .map_err(|error| single(dir, error.to_string()))?;
    let health = runtime
        .verify_runner(&prepared, &manifest, RunnerCheck::ContractAndHealth)
        .map_err(|error| error.to_string());
    Ok(ContractCheckReport { contract, health })
}

/// Runs one request against the package in `dir`, under the host's normal
/// limits, with `grants` resolved the same way a persisted directory grant
/// would be. `budget` bounds how long this call waits for the scheduler on
/// top of the host's own epoch deadline — it only matters if the worker is
/// busy, since a hung component is what the deadline itself is for.
pub fn simulate(
    dir: &Path,
    grants: &[SimulatedGrant],
    request: SimulatedRequest,
    budget: Duration,
) -> Result<SimulationReport, Vec<LocatedError>> {
    let manifest = validate_manifest_file(&dir.join("manifest.json"))?.manifest;
    if !manifest
        .manifest()
        .extensions
        .contains(&PluginExtension::Runner)
    {
        return Err(single(
            dir,
            "this manifest does not declare the runner extension; orivo-plugin-sdk can only \
             simulate runner-plugin today (see wit/README.md)"
                .to_string(),
        ));
    }
    let bytes = read_component(dir, &manifest)?;
    let runtime = PluginRuntime::new().map_err(|error| single(dir, error.to_string()))?;
    let sha256 = sha256_hex(&bytes);
    let prepared = runtime
        .prepare_component(&bytes, &sha256)
        .map_err(|error| single(dir, error.to_string()))?;

    let plugin_grants = if grants.is_empty() {
        PluginGrants::declared_only(&manifest)
    } else {
        let directories: BTreeMap<String, PathBuf> = grants
            .iter()
            .map(|grant| (grant.name.clone(), grant.path.clone()))
            .collect();
        let ids = directories.keys().cloned().collect();
        let grant = CapabilityGrant {
            plugin_id: manifest.id().to_string(),
            capability: PluginCapability::FilesRead,
            scope: CapabilityScope::DirectoryGrants(ids),
        };
        PluginGrants::resolve(&manifest, &[grant], &directories)
            .map_err(|error| single(dir, error.to_string()))?
    };

    let plugin_request = match request {
        SimulatedRequest::Identity => PluginRequest::Identity,
        SimulatedRequest::HealthCheck => PluginRequest::HealthCheck,
        SimulatedRequest::ValidateProfile {
            profile_id,
            display_name,
        } => PluginRequest::ValidateProfile {
            profile_id,
            display_name,
        },
        SimulatedRequest::DiscoverPage {
            profile_id,
            cursor,
            limit,
        } => PluginRequest::DiscoverPage {
            profile_id,
            cursor,
            limit,
        },
        SimulatedRequest::PrepareLaunch {
            profile_id,
            game_reference,
        } => PluginRequest::PrepareLaunch {
            profile_id,
            game_reference,
        },
    };

    let outcome = match runtime.submit(&prepared, manifest.id(), &plugin_grants, plugin_request) {
        Ok(handle) => match handle.wait_for(budget) {
            Ok(Ok(invocation)) => Ok(invocation.response),
            Ok(Err(job_error)) => Err(job_error.to_string()),
            Err(handle) => {
                handle.cancel();
                Err(format!(
                    "the job did not finish within {budget:?}; it has been cancelled"
                ))
            }
        },
        Err(submit_error) => Err(submit_error.to_string()),
    };

    Ok(SimulationReport {
        outcome,
        decisions: runtime.journal().entries(),
        plugin_messages: runtime.journal().plugin_messages(),
    })
}

fn read_component(
    dir: &Path,
    manifest: &orivo_lib::plugin_manifest::ValidatedPluginManifest,
) -> Result<Vec<u8>, Vec<LocatedError>> {
    let component_path = manifest
        .manifest()
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == orivo_lib::plugin_manifest::ArtifactKind::Component)
        .map(|artifact| dir.join(&artifact.path))
        .unwrap_or_else(|| dir.join("component.wasm"));
    std::fs::read(&component_path).map_err(|error| single(dir, format!("{}: {error}", component_path.display())))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn single(dir: &Path, message: String) -> Vec<LocatedError> {
    vec![LocatedError {
        location: dir.display().to_string(),
        message,
    }]
}
