//! The host half of a third-party runner: profiles, grants, imports, launches.
//!
//! A runner plugin knows a library format and nothing else. Everything that can
//! reach the machine stays here, and the division is the same one the WIT
//! contract describes:
//!
//! * **The plugin proposes; the host resolves.** `validate-profile` judges the
//!   profile the user built, `discover-page` proposes candidates as opaque
//!   external references, and `prepare-launch` returns a closed intent. None of
//!   the three can name a file. The emulation application comes from the
//!   profile the user picked through a native picker, and the game file is found
//!   by the host inside a granted folder — canonicalised, never followed out of
//!   scope, and re-checked immediately before the process starts.
//! * **A grant is a row, not a flag.** Folders live on the profile because they
//!   are the user's; the permission to read them lives in the catalog's grant
//!   ledger with the dates it was given and taken away. Revoking withdraws the
//!   permission and moves nothing else: the profile, its folders and every game
//!   already imported stay exactly where they were.
//! * **Every write is a whole page or none of it.** An import commits through
//!   [`CatalogStore`], which applies to a clone, validates the result and
//!   publishes it atomically, and it records the plugin's cursor as it goes so a
//!   cancelled or interrupted import resumes instead of walking the library
//!   again.
//!
//! What this module deliberately does not own: the component sandbox and its
//! ceilings (`plugin_runtime`), the queue those calls go through
//! (`plugin_scheduler`), and the package format (`plugin_installer`). It only
//! uses their public doors.

use crate::catalog::{
    Catalog, CatalogError, Game, GameSource, LaunchTarget, PluginGrantRecord,
    RunnerGameInventoryEntry, RunnerProfile, RunnerProfileStatus,
};
use crate::plugin_manifest::{
    CapabilityGrant, CapabilityScope, CompatibleVersionInfo, HostCompatibility,
    MAX_PLUGIN_ID_LENGTH, PluginCapability, PluginExtension, PluginManifest,
    ValidatedPluginManifest, valid_opaque_id,
};
use crate::plugin_runtime::{
    PluginDiscoveryPage, PluginGrants, PluginLaunchIntent, PluginLaunchMode,
    PluginProfileValidation, PluginRequest, PluginResponse, PluginRuntime, PluginRuntimeError,
    PreparedComponent, RunnerCheck,
};
use crate::plugin_scheduler::{JobError, JobHandle, SubmitError};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MANIFEST_FILE: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// How many entries the host will look at in one granted folder before it stops
/// looking. The same bound `host-files` scans behind, for the same reason: a
/// folder someone pointed at `/` must cost a bounded walk, not an unbounded one.
const MAX_DIRECTORY_SCAN: usize = 4_096;
/// A candidate's external id is what names a file inside a granted folder, so it
/// is held to the host's opaque-id grammar rather than to a filename's.
const MAX_EXTERNAL_ID_LENGTH: usize = 256;
/// How long a caller waits for a queued job beyond the deadline that stops the
/// component itself. A job can be behind another plugin's work, and waiting
/// forever for the queue is how a panel stops opening.
const QUEUE_WAIT_MULTIPLIER: u32 = 8;
/// How often a wait checks whether the user cancelled. Short enough to feel
/// immediate, long enough not to spin.
const WAIT_SLICE: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Every refusal the third-party runner host can produce. Each arm is a decision
/// the host made; `Plugin` is the only one carrying a component's own words, and
/// `plugin_runtime` has already bounded and stripped those.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerHostError {
    /// The plugin directory holds no usable package: no manifest, a manifest
    /// that fails the safety contract, or a component whose bytes disagree with
    /// it.
    PackageUnavailable(&'static str),
    /// The package is a plugin, but not a runner.
    NotARunner,
    /// The package was built for a different Orivo plugin SDK.
    Incompatible,
    /// The component answered its health check, and the answer was no.
    NotReady(Option<String>),
    /// No profile with that id, or one belonging to another plugin.
    UnknownProfile,
    /// The profile exists but is disabled, or the plugin has not accepted it.
    ProfileNotUsable,
    /// The plugin refused the profile. The message is the plugin's, sanitised.
    ProfileRefused(Option<String>),
    /// The permission this call needs is not in force: never granted, or
    /// revoked since. Nothing was deleted when it was revoked, so this is a
    /// refusal the user can undo.
    GrantMissing,
    /// A persisted grant no longer matches the manifest it was given under —
    /// the plugin stopped declaring the capability, or its scope grammar
    /// changed under an update.
    GrantRefused,
    /// The emulation application the profile names is gone, is not a file, or
    /// is not executable.
    ApplicationUnavailable,
    /// The candidate's external id does not name exactly one file inside a
    /// granted folder.
    GameUnresolvable,
    /// The file is no longer inside the folder it was granted under — including
    /// by way of a symbolic link that leaves it.
    GameOutsideScope,
    /// The library entry needs importing again before it can start.
    InventoryMissing,
    /// The user cancelled, or the host cancelled on their behalf.
    Cancelled,
    /// The plugin's own refusal, or a ceiling it ran into.
    Plugin(PluginRuntimeError),
    /// The plugin already has as much work queued as the host will hold.
    Busy,
    /// Orivo paused the plugin after repeated failures.
    Paused,
    /// The plugin runtime could not be reached at all.
    RuntimeUnavailable,
    /// A discovery pass handed back a cursor it had already used, so resuming
    /// it would never end.
    ImportStalled,
    /// The catalog could not be read or written.
    Catalog(String),
}

impl std::fmt::Display for RunnerHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PackageUnavailable(reason) => {
                write!(formatter, "This runner plugin is unusable ({reason}).")
            }
            Self::NotARunner => write!(
                formatter,
                "This plugin does not implement Orivo's runner contract."
            ),
            Self::Incompatible => {
                write!(formatter, "This plugin requires a different Orivo version.")
            }
            Self::NotReady(Some(message)) => write!(formatter, "{message}"),
            Self::NotReady(None) => write!(
                formatter,
                "This runner plugin reports that it is not ready yet."
            ),
            Self::UnknownProfile => write!(
                formatter,
                "This runner profile is no longer available. Review its setup and try again."
            ),
            Self::ProfileNotUsable => write!(
                formatter,
                "This runner profile is disabled or has not been accepted by its plugin yet."
            ),
            Self::ProfileRefused(Some(message)) => write!(formatter, "{message}"),
            Self::ProfileRefused(None) => write!(
                formatter,
                "This runner plugin did not accept the profile as configured."
            ),
            Self::GrantMissing => write!(
                formatter,
                "This runner no longer has permission to use that folder. Allow it again to continue."
            ),
            Self::GrantRefused => write!(
                formatter,
                "This plugin's permissions no longer match what its package declares. Set it up again."
            ),
            Self::ApplicationUnavailable => write!(
                formatter,
                "The emulator this profile points at is missing or cannot be started. Choose it again."
            ),
            Self::GameUnresolvable => write!(
                formatter,
                "Orivo could not find exactly one game file for this entry in the allowed folders."
            ),
            Self::GameOutsideScope => write!(
                formatter,
                "This game file is outside the folder it was allowed in."
            ),
            Self::InventoryMissing => write!(
                formatter,
                "This game needs to be imported again before it can start."
            ),
            Self::Cancelled => write!(formatter, "The runner operation was cancelled."),
            Self::Plugin(error) => write!(formatter, "{error}"),
            Self::Busy => write!(
                formatter,
                "This plugin already has as much work queued as Orivo will hold."
            ),
            Self::Paused => write!(
                formatter,
                "Orivo paused this plugin after repeated failures. Resume it to try again."
            ),
            Self::RuntimeUnavailable => {
                write!(formatter, "The Orivo plugin runtime is unavailable.")
            }
            Self::ImportStalled => write!(
                formatter,
                "This plugin repeated a discovery cursor, so Orivo stopped the import."
            ),
            Self::Catalog(_) => write!(formatter, "The game catalog is temporarily unavailable."),
        }
    }
}

impl std::error::Error for RunnerHostError {}

impl From<CatalogError> for RunnerHostError {
    fn from(error: CatalogError) -> Self {
        Self::Catalog(error.to_string())
    }
}

// ---------------------------------------------------------------------------
// The installed package
// ---------------------------------------------------------------------------

/// One installed runner plugin, verified and compiled, ready to be called.
///
/// Loading re-reads and re-hashes the component immediately before Wasmtime
/// sees it and then makes the package's three accounts of itself agree — the
/// manifest, the component's type, and what the component says when asked — by
/// way of `verify_runner`. A package that fails any of that never becomes a
/// `RunnerPackage`, so nothing downstream has to wonder.
#[derive(Clone)]
pub struct RunnerPackage {
    runtime: PluginRuntime,
    manifest: ValidatedPluginManifest,
    prepared: PreparedComponent,
}

impl std::fmt::Debug for RunnerPackage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunnerPackage")
            .field("plugin_id", &self.manifest.id())
            .finish_non_exhaustive()
    }
}

impl RunnerPackage {
    pub fn load(
        runtime: &PluginRuntime,
        plugin_root: &Path,
        plugin_id: &str,
        compatibility: HostCompatibility,
    ) -> Result<Self, RunnerHostError> {
        // The id names a directory, so it is held to the opaque grammar before
        // it is joined onto anything: a value starting with `.` or carrying a
        // separator never becomes a path component.
        if !valid_opaque_id(plugin_id, MAX_PLUGIN_ID_LENGTH) {
            return Err(RunnerHostError::PackageUnavailable("unknown plugin"));
        }
        let directory = plugin_root.join(plugin_id);
        let manifest_bytes = read_bounded_file(&directory.join(MANIFEST_FILE), MAX_MANIFEST_BYTES)
            .map_err(|_| RunnerHostError::PackageUnavailable("manifest unavailable"))?;
        let manifest = serde_json::from_slice::<PluginManifest>(&manifest_bytes)
            .map_err(|_| RunnerHostError::PackageUnavailable("manifest is not valid JSON"))?
            .validate()
            .map_err(|_| {
                RunnerHostError::PackageUnavailable("manifest fails the safety contract")
            })?;
        if manifest.id() != plugin_id {
            return Err(RunnerHostError::PackageUnavailable(
                "the folder does not match the manifest identity",
            ));
        }
        if !manifest
            .manifest()
            .extensions
            .contains(&PluginExtension::Runner)
        {
            return Err(RunnerHostError::NotARunner);
        }
        match compatibility.compatibility_for(&manifest) {
            CompatibleVersionInfo::Compatible => {}
            CompatibleVersionInfo::UnsupportedSdk | CompatibleVersionInfo::RequiresNewerOrivo => {
                return Err(RunnerHostError::Incompatible);
            }
        }

        let artifact = manifest
            .manifest()
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == crate::plugin_manifest::ArtifactKind::Component)
            .ok_or(RunnerHostError::PackageUnavailable(
                "the package declares no component",
            ))?;
        let bytes = read_bounded_file(&directory.join(&artifact.path), artifact.byte_size)
            .map_err(|_| RunnerHostError::PackageUnavailable("component unavailable"))?;
        if bytes.len() as u64 != artifact.byte_size {
            return Err(RunnerHostError::PackageUnavailable(
                "the component does not match its declared size",
            ));
        }
        let sha256 = sha256_of(&bytes);
        if !sha256.eq_ignore_ascii_case(&artifact.sha256) {
            return Err(RunnerHostError::PackageUnavailable(
                "the component does not match its declared hash",
            ));
        }
        let prepared = runtime
            .prepare_component(&bytes, &sha256)
            .map_err(RunnerHostError::Plugin)?;
        let health = runtime
            .verify_runner(&prepared, &manifest, RunnerCheck::ContractAndHealth)
            .map_err(map_runtime_error)?;
        if let Some(health) = health
            && !health.ready
        {
            return Err(RunnerHostError::NotReady(health.message));
        }
        Ok(Self {
            runtime: runtime.clone(),
            manifest,
            prepared,
        })
    }

    pub fn plugin_id(&self) -> &str {
        self.manifest.id()
    }

    pub fn manifest(&self) -> &ValidatedPluginManifest {
        &self.manifest
    }

    /// Queue one invocation and wait for it, cancelling on the way out.
    ///
    /// The epoch deadline already stops a component that will not return, but
    /// the job can also still be queued behind another plugin's work, so the
    /// wall-clock wait is bounded separately. Cancellation is checked on every
    /// slice and reaches a call already inside Wasmtime, because the token the
    /// scheduler handed the job is the one the host is holding.
    fn call(
        &self,
        grants: &PluginGrants,
        request: PluginRequest,
        budget: Duration,
        cancelled: &AtomicBool,
    ) -> Result<PluginResponse, RunnerHostError> {
        let mut handle: JobHandle<_> = self
            .runtime
            .submit(&self.prepared, self.plugin_id(), grants, request)
            .map_err(|error| match error {
                SubmitError::Busy { .. } => RunnerHostError::Busy,
                SubmitError::Degraded { .. } => RunnerHostError::Paused,
                SubmitError::ShuttingDown => RunnerHostError::RuntimeUnavailable,
            })?;
        let deadline = Instant::now() + budget;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                handle.cancel();
                return Err(RunnerHostError::Cancelled);
            }
            match handle.wait_for(WAIT_SLICE) {
                Ok(Ok(invocation)) => return Ok(invocation.response),
                Ok(Err(JobError::Cancelled)) => return Err(RunnerHostError::Cancelled),
                Ok(Err(JobError::Runtime(error))) => return Err(map_runtime_error(error)),
                Ok(Err(JobError::Abandoned)) => return Err(RunnerHostError::RuntimeUnavailable),
                Ok(Err(JobError::Panicked)) => {
                    return Err(RunnerHostError::Plugin(PluginRuntimeError::Trapped));
                }
                Err(pending) => {
                    if Instant::now() >= deadline {
                        pending.cancel();
                        return Err(RunnerHostError::Plugin(
                            PluginRuntimeError::DeadlineExceeded,
                        ));
                    }
                    handle = pending;
                }
            }
        }
    }

    fn interactive_budget(&self) -> Duration {
        self.runtime
            .limits()
            .interactive_deadline
            .saturating_mul(QUEUE_WAIT_MULTIPLIER)
    }

    fn discovery_budget(&self) -> Duration {
        self.runtime
            .limits()
            .discovery_deadline
            .saturating_mul(QUEUE_WAIT_MULTIPLIER)
    }
}

fn map_runtime_error(error: PluginRuntimeError) -> RunnerHostError {
    match error {
        PluginRuntimeError::MissingWorld => RunnerHostError::NotARunner,
        PluginRuntimeError::Cancelled => RunnerHostError::Cancelled,
        PluginRuntimeError::Busy => RunnerHostError::Busy,
        PluginRuntimeError::Paused => RunnerHostError::Paused,
        PluginRuntimeError::EngineUnavailable => RunnerHostError::RuntimeUnavailable,
        other => RunnerHostError::Plugin(other),
    }
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

/// The grants in force for one plugin *on one profile*.
///
/// Resolving per profile is what makes the ledger usable at all: two profiles of
/// the same plugin name their folders under the same slot — the v1 `runner`
/// world gives a component no way to declare which slots it will ask for, so it
/// hard-codes them — and a grant row that listed both would resolve to whichever
/// folder came first. Narrowing the persisted scope to the profile being invoked
/// keeps the answer unambiguous, and it means an invocation for one profile can
/// never reach another profile's folder.
pub fn resolve_profile_grants(
    catalog: &Catalog,
    manifest: &ValidatedPluginManifest,
    profile: &RunnerProfile,
) -> Result<PluginGrants, RunnerHostError> {
    let directories = profile
        .game_directories
        .iter()
        .map(|directory| (directory.id.clone(), directory.path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut grants = Vec::new();
    for record in catalog
        .plugin_grants
        .iter()
        .filter(|record| record.plugin_id == profile.plugin_id && record.is_active())
    {
        let grant = record.to_capability_grant();
        match (&grant.capability, &grant.scope) {
            (PluginCapability::FilesRead, CapabilityScope::DirectoryGrants(ids)) => {
                let narrowed = ids
                    .iter()
                    .filter(|id| directories.contains_key(*id))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                if narrowed.is_empty() {
                    continue;
                }
                grants.push(CapabilityGrant {
                    scope: CapabilityScope::DirectoryGrants(narrowed),
                    ..grant
                });
            }
            (PluginCapability::RunnerPrepare, CapabilityScope::RunnerProfiles(ids)) => {
                if !ids.contains(&profile.id) {
                    continue;
                }
                grants.push(CapabilityGrant {
                    scope: CapabilityScope::RunnerProfiles(BTreeSet::from([profile.id.clone()])),
                    ..grant
                });
            }
            _ => grants.push(grant),
        }
    }
    PluginGrants::resolve(manifest, &grants, &directories)
        .map_err(|_| RunnerHostError::GrantRefused)
}

/// Whether one granted folder is currently readable by its plugin. A launch
/// asks this before it resolves a file: the folders stay on the profile when a
/// grant is withdrawn — the games inside them are still the user's — so the
/// ledger, not the profile, is what says whether Orivo may still reach in.
pub fn directory_grant_is_active(catalog: &Catalog, plugin_id: &str, directory_id: &str) -> bool {
    catalog
        .active_plugin_grant(plugin_id, PluginCapability::FilesRead)
        .is_some_and(|grant| match &grant.scope {
            CapabilityScope::DirectoryGrants(ids) => ids.contains(directory_id),
            _ => false,
        })
}

/// The grant rows that a newly granted folder implies, as one statement of what
/// the plugin may now reach. Granting is always complete rather than additive,
/// so a row can never drift out of step with the profiles it scopes.
pub fn directory_grant_records(
    catalog: &Catalog,
    plugin_id: &str,
    granted_at: u64,
) -> Vec<PluginGrantRecord> {
    let profiles = catalog.runner_profiles_for_plugin(plugin_id);
    let directories = profiles
        .iter()
        .flat_map(|profile| profile.game_directories.iter())
        .map(|directory| directory.id.clone())
        .collect::<BTreeSet<_>>();
    let profile_ids = profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<BTreeSet<_>>();
    let mut records = Vec::new();
    if !directories.is_empty() {
        records.push(PluginGrantRecord {
            plugin_id: plugin_id.to_owned(),
            capability: PluginCapability::FilesRead,
            scope: CapabilityScope::DirectoryGrants(directories),
            granted_at,
            revoked_at: None,
        });
    }
    if !profile_ids.is_empty() {
        records.push(PluginGrantRecord {
            plugin_id: plugin_id.to_owned(),
            capability: PluginCapability::RunnerPrepare,
            scope: CapabilityScope::RunnerProfiles(profile_ids),
            granted_at,
            revoked_at: None,
        });
    }
    records
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// Ask the owning plugin whether it accepts a profile.
///
/// This is the whole of what the v1 contract lets a plugin judge: the WIT
/// `runner-profile` record carries an id and a display name, and nothing else.
/// The emulation application, the granted folders and the launch mode are the
/// host's to validate, and it does — which is why a profile the plugin accepted
/// is still not a profile that can start anything until the rest checks out.
pub fn validate_profile_with_plugin(
    package: &RunnerPackage,
    catalog: &Catalog,
    profile: &RunnerProfile,
    cancelled: &AtomicBool,
) -> Result<PluginProfileValidation, RunnerHostError> {
    let grants = resolve_profile_grants(catalog, package.manifest(), profile)?;
    let response = package.call(
        &grants,
        PluginRequest::ValidateProfile {
            profile_id: profile.id.clone(),
            display_name: profile.display_name.clone(),
        },
        package.interactive_budget(),
        cancelled,
    )?;
    match response {
        PluginResponse::ProfileValidation(validation) => Ok(validation),
        _ => Err(RunnerHostError::Plugin(PluginRuntimeError::InvalidResult(
            "profile validation",
        ))),
    }
}

/// Apply a plugin's verdict to a profile. A refusal keeps the profile and its
/// folders — the user chose those — and records why, so the next attempt starts
/// from what they already built rather than from nothing.
pub fn apply_profile_validation(profile: &mut RunnerProfile, validation: &PluginProfileValidation) {
    if validation.valid {
        profile.status = RunnerProfileStatus::Valid;
        profile.status_message = None;
    } else {
        profile.status = RunnerProfileStatus::Rejected;
        profile.status_message = validation.message.clone();
    }
}

fn usable_profile<'catalog>(
    catalog: &'catalog Catalog,
    plugin_id: &str,
    profile_id: &str,
) -> Result<&'catalog RunnerProfile, RunnerHostError> {
    let profile = catalog
        .runner_profile(profile_id)
        .filter(|profile| profile.plugin_id == plugin_id)
        .ok_or(RunnerHostError::UnknownProfile)?;
    match profile.status {
        RunnerProfileStatus::Valid => {}
        // The plugin's own words are the actionable half of this refusal, so
        // they are what the user is shown rather than a generic sentence.
        RunnerProfileStatus::Rejected => {
            return Err(RunnerHostError::ProfileRefused(
                profile.status_message.clone(),
            ));
        }
        RunnerProfileStatus::Unvalidated => return Err(RunnerHostError::ProfileNotUsable),
    }
    if !profile.enabled {
        return Err(RunnerHostError::ProfileNotUsable);
    }
    Ok(profile)
}

// ---------------------------------------------------------------------------
// Resolving what the plugin is not allowed to name
// ---------------------------------------------------------------------------

/// Find the one file a candidate's external id names inside the profile's
/// granted folders, and say which folder it was found in.
///
/// The rule is the host's and it is deliberately narrow: a match is a file whose
/// name, or whose name without its final extension, is exactly the external id.
/// Nothing is recursive, symbolic links are not followed, and an id that matches
/// in more than one place is refused rather than guessed — an ambiguous
/// resolution is how a runner ends up starting a different game than the one the
/// user picked.
pub fn resolve_game_file(
    profile: &RunnerProfile,
    external_id: &str,
) -> Result<(String, PathBuf), RunnerHostError> {
    if !valid_opaque_id(external_id, MAX_EXTERNAL_ID_LENGTH) {
        return Err(RunnerHostError::GameUnresolvable);
    }
    let mut matches = Vec::new();
    for directory in &profile.game_directories {
        let Ok(root) = fs::canonicalize(&directory.path) else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.take(MAX_DIRECTORY_SCAN).filter_map(Result::ok) {
            // `file_type` here comes from the directory entry, so a symbolic
            // link reports as one instead of as whatever it points at.
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
            if name != external_id && stem != external_id {
                continue;
            }
            let path = entry.path();
            if let Ok(canonical) = canonical_file_inside(&path, &root) {
                matches.push((directory.id.clone(), canonical));
            }
        }
    }
    matches.sort();
    matches.dedup();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        _ => Err(RunnerHostError::GameUnresolvable),
    }
}

/// Re-check the file an inventory entry already names, immediately before it is
/// handed to a process.
///
/// Import-time resolution is not a standing authorisation: the entry may be
/// weeks old, the file may have become a symbolic link out of the folder, and
/// the grant may have been withdrawn since. All three are asked again here.
pub fn reverify_game_file(
    catalog: &Catalog,
    profile: &RunnerProfile,
    entry: &RunnerGameInventoryEntry,
) -> Result<PathBuf, RunnerHostError> {
    if !directory_grant_is_active(catalog, &profile.plugin_id, &entry.directory_grant_id) {
        return Err(RunnerHostError::GrantMissing);
    }
    let directory = profile
        .granted_directory(&entry.directory_grant_id)
        .ok_or(RunnerHostError::GrantMissing)?;
    let root = fs::canonicalize(&directory.path).map_err(|_| RunnerHostError::GameOutsideScope)?;
    canonical_file_inside(&entry.game_path, &root)
}

/// A regular file, not a link, strictly inside `root` after canonicalisation.
///
/// Both halves matter. Refusing the link itself stops a file that was replaced
/// in place; comparing the canonical path against the canonical root stops one
/// reached through a link further up the chain.
fn canonical_file_inside(path: &Path, root: &Path) -> Result<PathBuf, RunnerHostError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RunnerHostError::GameOutsideScope)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(RunnerHostError::GameOutsideScope);
    }
    let canonical = fs::canonicalize(path).map_err(|_| RunnerHostError::GameOutsideScope)?;
    if canonical == root || !canonical.starts_with(root) {
        return Err(RunnerHostError::GameOutsideScope);
    }
    Ok(canonical)
}

/// Resolve the emulation application to the file a process can start. A macOS
/// bundle is resolved through its own `Info.plist`, exactly as a locally
/// imported game is, and the result still has to be a regular executable file.
pub fn resolve_application(profile: &RunnerProfile) -> Result<PathBuf, RunnerHostError> {
    let executable = crate::catalog::resolve_executable(&profile.application)
        .map_err(|_| RunnerHostError::ApplicationUnavailable)?;
    let executable =
        fs::canonicalize(executable).map_err(|_| RunnerHostError::ApplicationUnavailable)?;
    if !executable.is_file() || !is_executable(&executable) {
        return Err(RunnerHostError::ApplicationUnavailable);
    }
    Ok(executable)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// A third-party runner launch the host has fully resolved: a program, a fixed
/// argument list and a working directory. Nothing in it came from the plugin —
/// the intent only said *which* profile and *which* game reference, and both had
/// to be the ones the host asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRunnerLaunch {
    program: PathBuf,
    arguments: Vec<PathBuf>,
    working_directory: Option<PathBuf>,
    title: String,
}

impl PreparedRunnerLaunch {
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The process, built without a shell. Every value here is a canonical path
    /// the host resolved itself, passed as one argument each, so no quoting,
    /// splitting or interpolation happens anywhere in this path.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(directory) = self.working_directory.as_ref() {
            command.current_dir(directory);
        }
        command
    }

    pub fn spawn(&self) -> Result<Child, RunnerHostError> {
        self.command()
            .spawn()
            .map_err(|_| RunnerHostError::ApplicationUnavailable)
    }
}

/// Turn a runner launch target into a process the host owns.
///
/// The order is the contract: the profile has to be one the plugin accepted, the
/// permission to reach the folder has to still be in force, the plugin gets to
/// prepare an intent under the interactive budget, the intent has to be about
/// the call that was made, and only then does the host resolve an application
/// and a file. A plugin that refuses, lies or hangs stops the launch at its own
/// step, and none of those steps can produce a command line.
pub fn prepare_runner_launch(
    package: &RunnerPackage,
    catalog: &Catalog,
    profile_id: &str,
    game_ref: &str,
    cancelled: &AtomicBool,
) -> Result<PreparedRunnerLaunch, RunnerHostError> {
    let profile = usable_profile(catalog, package.plugin_id(), profile_id)?;
    let entry = catalog
        .runner_inventory_entry(profile_id, game_ref)
        .ok_or(RunnerHostError::InventoryMissing)?;
    let grants = resolve_profile_grants(catalog, package.manifest(), profile)?;
    // `runner.prepare` is what the user agreed to when they created the profile.
    // Without it the plugin is installed and identified and nothing more, so the
    // host does not ask it to prepare anything.
    if !grants.holds(PluginCapability::RunnerPrepare) {
        return Err(RunnerHostError::GrantMissing);
    }
    let game_file = reverify_game_file(catalog, profile, entry)?;
    let application = resolve_application(profile)?;

    let response = package.call(
        &grants,
        PluginRequest::PrepareLaunch {
            profile_id: profile_id.to_owned(),
            game_reference: game_ref.to_owned(),
        },
        package.interactive_budget(),
        cancelled,
    )?;
    let intent: PluginLaunchIntent = match response {
        PluginResponse::LaunchIntent(intent) => intent,
        _ => {
            return Err(RunnerHostError::Plugin(PluginRuntimeError::InvalidResult(
                "launch intent",
            )));
        }
    };
    // `plugin_runtime` already refused an intent naming another runner, another
    // profile, another game or an unknown mode. Asking again here is cheap and
    // keeps this function correct on its own terms rather than on another
    // module's.
    if intent.runner_id() != package.plugin_id()
        || intent.profile_id() != profile_id
        || intent.game_reference() != game_ref
    {
        return Err(RunnerHostError::Plugin(PluginRuntimeError::InvalidResult(
            "intent target",
        )));
    }

    match intent.mode() {
        // The one mode the v1 contract declares: the application is started in
        // its own directory with the resolved game file as its single argument.
        // A second mode is an ABI decision, not a plugin's, which is why this
        // match is exhaustive rather than defaulted.
        PluginLaunchMode::Default => Ok(PreparedRunnerLaunch {
            working_directory: application.parent().map(Path::to_path_buf),
            program: application,
            arguments: vec![game_file],
            title: entry.title.clone(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Transactional catalog writes
// ---------------------------------------------------------------------------

/// The one door every runner write goes through.
///
/// A mutation is applied to a clone, validated as a whole catalog and published
/// atomically before the in-memory copy is replaced, so a rejected page leaves
/// the library exactly as it was and a crash mid-write leaves the previous file.
/// The mutation lease is the same one the rest of the backend takes, which is
/// what keeps an import and a launch from interleaving on the same profile.
#[derive(Clone)]
pub struct CatalogStore {
    catalog: Arc<RwLock<Catalog>>,
    path: PathBuf,
    mutation: Arc<Mutex<()>>,
}

impl CatalogStore {
    pub fn new(catalog: Arc<RwLock<Catalog>>, path: PathBuf, mutation: Arc<Mutex<()>>) -> Self {
        Self {
            catalog,
            path,
            mutation,
        }
    }

    pub fn snapshot(&self) -> Result<Catalog, RunnerHostError> {
        self.catalog
            .read()
            .map(|catalog| catalog.clone())
            .map_err(|_| RunnerHostError::Catalog("the catalog lock is poisoned".into()))
    }

    pub fn commit<T>(
        &self,
        apply: impl FnOnce(&mut Catalog) -> Result<T, CatalogError>,
    ) -> Result<T, RunnerHostError> {
        let _lease = self
            .mutation
            .lock()
            .map_err(|_| RunnerHostError::Catalog("the catalog lock is poisoned".into()))?;
        let mut next = self.snapshot()?;
        let outcome = apply(&mut next)?;
        next.validate()?;
        next.save_atomically(&self.path)?;
        let mut current = self
            .catalog
            .write()
            .map_err(|_| RunnerHostError::Catalog("the catalog lock is poisoned".into()))?;
        *current = next;
        Ok(outcome)
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerImportLimits {
    /// How many candidates one `discover-page` may return. Small enough that a
    /// page is a transaction rather than a library.
    pub page_size: u32,
    pub max_pages: usize,
    pub max_games: usize,
}

impl Default for RunnerImportLimits {
    fn default() -> Self {
        Self {
            page_size: 50,
            max_pages: 256,
            max_games: 20_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunnerImportProgress {
    pub pages: usize,
    pub imported: usize,
    pub refreshed: usize,
    /// Candidates the host would not write down: an external id naming nothing
    /// inside a granted folder, or naming more than one thing.
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerImportOutcome {
    pub progress: RunnerImportProgress,
    /// The cursor this run started from, if it resumed one.
    pub resumed_from: Option<String>,
    pub complete: bool,
    pub cancelled: bool,
}

/// Walk a runner's library one page at a time and write each page down.
///
/// Three properties are worth naming because they are what the plan asks for.
/// The write is *transactional*: a page is one [`CatalogStore::commit`], and a
/// candidate the catalog refuses is dropped from that page rather than taking it
/// down. It is *idempotent*: a card is keyed by the plugin's external reference,
/// so importing the same library twice refreshes rather than duplicates. And it
/// is *resumable*: the cursor the plugin returned is persisted with the page it
/// belongs to, so a cancelled import — or one interrupted by quitting Orivo —
/// continues from there instead of walking the library again.
pub fn import_runner_games(
    package: &RunnerPackage,
    store: &CatalogStore,
    profile_id: &str,
    limits: RunnerImportLimits,
    cancelled: &AtomicBool,
    mut on_progress: impl FnMut(RunnerImportProgress),
) -> Result<RunnerImportOutcome, RunnerHostError> {
    let catalog = store.snapshot()?;
    let profile = usable_profile(&catalog, package.plugin_id(), profile_id)?.clone();
    let grants = resolve_profile_grants(&catalog, package.manifest(), &profile)?;
    // Discovery reads the granted folder through `host-files`, so without the
    // permission in force there is nothing for the plugin to page through.
    if !grants.holds(PluginCapability::FilesRead) {
        return Err(RunnerHostError::GrantMissing);
    }

    // A finished import starts again from the beginning: its cursor is spent,
    // and re-walking is how a library that gained files is noticed at all.
    let mut cursor = (!profile.import_complete)
        .then(|| profile.import_cursor.clone())
        .flatten();
    let resumed_from = cursor.clone();
    let mut seen_cursors = BTreeSet::new();
    let mut progress = RunnerImportProgress::default();
    let mut complete = false;
    let mut was_cancelled = false;

    for _ in 0..limits.max_pages {
        if cancelled.load(Ordering::Relaxed) {
            was_cancelled = true;
            break;
        }
        let page = discover_page(
            package,
            &grants,
            profile_id,
            cursor.clone(),
            limits.page_size,
            cancelled,
        )?;
        let next_cursor = page.next_cursor.clone();
        if let Some(next) = next_cursor.as_deref()
            && !seen_cursors.insert(next.to_owned())
        {
            return Err(RunnerHostError::ImportStalled);
        }

        let committed = commit_page(store, package.plugin_id(), profile_id, &page)?;
        progress.pages += 1;
        progress.imported += committed.imported;
        progress.refreshed += committed.refreshed;
        progress.skipped += committed.skipped;
        on_progress(progress);

        complete = page.complete;
        if complete || next_cursor.is_none() {
            break;
        }
        if progress.imported + progress.refreshed >= limits.max_games {
            break;
        }
        cursor = next_cursor;
    }

    Ok(RunnerImportOutcome {
        progress,
        resumed_from,
        complete,
        cancelled: was_cancelled,
    })
}

fn discover_page(
    package: &RunnerPackage,
    grants: &PluginGrants,
    profile_id: &str,
    cursor: Option<String>,
    limit: u32,
    cancelled: &AtomicBool,
) -> Result<PluginDiscoveryPage, RunnerHostError> {
    let response = package.call(
        grants,
        PluginRequest::DiscoverPage {
            profile_id: profile_id.to_owned(),
            cursor,
            limit,
        },
        package.discovery_budget(),
        cancelled,
    )?;
    match response {
        PluginResponse::DiscoveryPage(page) => Ok(page),
        _ => Err(RunnerHostError::Plugin(PluginRuntimeError::InvalidResult(
            "discovery page",
        ))),
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct CommittedPage {
    imported: usize,
    refreshed: usize,
    skipped: usize,
}

/// One page, one transaction. The cursor moves with the candidates it describes,
/// so a crash between two pages can only ever lose the page that was in flight.
fn commit_page(
    store: &CatalogStore,
    plugin_id: &str,
    profile_id: &str,
    page: &PluginDiscoveryPage,
) -> Result<CommittedPage, RunnerHostError> {
    let imported_at = unix_millis();
    store.commit(|catalog| {
        let profile = catalog
            .runner_profile(profile_id)
            .filter(|profile| profile.plugin_id == plugin_id)
            .cloned()
            .ok_or_else(|| {
                CatalogError::Invalid("the runner profile changed during the import".into())
            })?;
        let mut committed = CommittedPage::default();
        for candidate in &page.games {
            let Ok((directory_grant_id, game_path)) =
                resolve_game_file(&profile, &candidate.external_id)
            else {
                committed.skipped += 1;
                continue;
            };
            let entry = RunnerGameInventoryEntry {
                profile_id: profile_id.to_owned(),
                game_ref: candidate.external_id.clone(),
                title: candidate.title.clone(),
                provider_id: candidate.provider_id.clone(),
                external_id: candidate.external_id.clone(),
                game_path,
                directory_grant_id,
                platform: candidate.platform.clone(),
                imported_at: Some(imported_at),
            };
            let game = runner_catalog_game(plugin_id, profile_id, &entry);
            // A candidate the catalog refuses is dropped from this page, not
            // allowed to take the page with it: one unusable ROM must not stop a
            // library from importing.
            let mut trial = catalog.clone();
            let Ok(inserted) = trial.upsert_runner_inventory(entry) else {
                committed.skipped += 1;
                continue;
            };
            if trial.upsert_runner(game).is_err() || trial.validate().is_err() {
                committed.skipped += 1;
                continue;
            }
            *catalog = trial;
            if inserted {
                committed.imported += 1;
            } else {
                committed.refreshed += 1;
            }
        }
        let mut profile = profile;
        profile.import_cursor = page.next_cursor.clone();
        profile.import_complete = page.complete;
        profile.last_imported_at = Some(imported_at);
        catalog.upsert_runner_profile(profile)?;
        Ok(committed)
    })
}

/// The library card for one imported third-party runner game. Every launch field
/// is deliberately empty: a runner card carries opaque references, and the host
/// resolves the rest from the profile the user created.
pub fn runner_catalog_game(
    plugin_id: &str,
    profile_id: &str,
    entry: &RunnerGameInventoryEntry,
) -> Game {
    Game {
        id: runner_game_id(plugin_id, profile_id, &entry.game_ref),
        title: entry.title.clone(),
        executable_path: None,
        source: GameSource::Local,
        source_id: None,
        launch_target: LaunchTarget::Runner {
            runner_id: plugin_id.to_owned(),
            game_ref: entry.game_ref.clone(),
            profile_id: profile_id.to_owned(),
        },
        installation_path: None,
        working_directory: None,
        arguments: Vec::new(),
        description: None,
        metadata: entry.platform.clone(),
        artwork_path: None,
        artwork_source_path: None,
        cover_path: None,
        cover_source_path: None,
        home_image_path: None,
        landscape_image_path: None,
        logo_path: None,
        hidden: false,
        hero_video_path: None,
        last_played_at: None,
        play_time_seconds: 0,
        extra: BTreeMap::new(),
    }
}

/// Derived from the same three opaque values the launch target carries, so a
/// second import of the same external game lands on the same card rather than
/// beside it.
pub fn runner_game_id(plugin_id: &str, profile_id: &str, game_ref: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(plugin_id.as_bytes());
    digest.update(b"\0");
    digest.update(profile_id.as_bytes());
    digest.update(b"\0");
    digest.update(game_ref.as_bytes());
    format!("runner:{plugin_id}:{profile_id}:{:x}", digest.finalize())
}

pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn read_bounded_file(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > max_bytes
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unsafe file"));
    }
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file grew while reading",
        ));
    }
    Ok(bytes)
}

fn sha256_of(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{RunnerGrantedDirectory, RunnerProfileSettings};
    use std::sync::atomic::AtomicU64;

    fn temporary_root(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-runner-host-{tag}-{}-{}-{}",
            std::process::id(),
            unix_millis(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn profile_over(root: &Path) -> RunnerProfile {
        RunnerProfile {
            id: "fixture-profile-1".into(),
            plugin_id: "com.orivo.fixture-runner".into(),
            display_name: "Fixture".into(),
            application: root.join("Emulator"),
            game_directories: vec![RunnerGrantedDirectory {
                id: "fixture-games".into(),
                path: root.join("games"),
            }],
            settings: RunnerProfileSettings::default(),
            status: RunnerProfileStatus::Valid,
            status_message: None,
            enabled: true,
            import_cursor: None,
            import_complete: false,
            last_imported_at: None,
        }
    }

    #[test]
    fn resolves_a_candidate_by_its_name_or_by_its_name_without_an_extension() {
        let root = temporary_root("resolve");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::write(root.join("games/alpha.rom"), b"Alpha").unwrap();
        fs::write(root.join("games/beta"), b"Beta").unwrap();
        let profile = profile_over(&root);

        let (grant, path) = resolve_game_file(&profile, "alpha").unwrap();
        assert_eq!(grant, "fixture-games");
        assert_eq!(path.file_name().unwrap(), "alpha.rom");
        assert_eq!(
            resolve_game_file(&profile, "beta").unwrap().1.file_name(),
            Some(std::ffi::OsStr::new("beta"))
        );

        fs::remove_dir_all(root).unwrap();
    }

    /// Two files whose stems collide is the one case where the host cannot say
    /// which game the user meant, and starting the wrong one is worse than
    /// importing neither.
    #[test]
    fn refuses_a_candidate_that_names_more_than_one_file() {
        let root = temporary_root("ambiguous");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::write(root.join("games/alpha.rom"), b"Alpha").unwrap();
        fs::write(root.join("games/alpha.bin"), b"Alpha").unwrap();

        assert_eq!(
            resolve_game_file(&profile_over(&root), "alpha"),
            Err(RunnerHostError::GameUnresolvable)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The external id is what a plugin *chose*, so it is held to the host's
    /// opaque grammar before it is ever joined onto a granted folder.
    #[test]
    fn refuses_an_external_id_that_is_really_a_path() {
        let root = temporary_root("traversal");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::write(root.join("secret.txt"), b"a keychain token").unwrap();
        let profile = profile_over(&root);

        for external_id in ["../secret.txt", "..", "games/alpha", "/etc/passwd", ""] {
            assert_eq!(
                resolve_game_file(&profile, external_id),
                Err(RunnerHostError::GameUnresolvable),
                "{external_id} should not resolve"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_out_of_a_granted_folder_is_never_resolved() {
        let root = temporary_root("symlink");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::write(root.join("secret.txt"), b"a keychain token").unwrap();
        std::os::unix::fs::symlink(root.join("secret.txt"), root.join("games/escape.rom")).unwrap();
        let profile = profile_over(&root);

        // The listing half: a link is not a file the host will import.
        assert_eq!(
            resolve_game_file(&profile, "escape"),
            Err(RunnerHostError::GameUnresolvable)
        );
        // And the launch half, where the entry already exists and the file was
        // swapped underneath it.
        let entry = RunnerGameInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: "escape".into(),
            title: "Escape".into(),
            provider_id: profile.plugin_id.clone(),
            external_id: "escape".into(),
            game_path: root.join("games/escape.rom"),
            directory_grant_id: "fixture-games".into(),
            platform: None,
            imported_at: None,
        };
        let mut catalog = Catalog::default();
        catalog
            .grant_plugin_capability(PluginGrantRecord {
                plugin_id: profile.plugin_id.clone(),
                capability: PluginCapability::FilesRead,
                scope: CapabilityScope::DirectoryGrants(BTreeSet::from(["fixture-games".into()])),
                granted_at: 1,
                revoked_at: None,
            })
            .unwrap();
        assert_eq!(
            reverify_game_file(&catalog, &profile, &entry),
            Err(RunnerHostError::GameOutsideScope)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The card id has to come from the three opaque values alone, or a second
    /// import of the same game would land beside the first instead of on it.
    #[test]
    fn a_card_id_is_derived_only_from_the_launch_target() {
        let first = runner_game_id("com.orivo.fixture-runner", "fixture-profile-1", "alpha");
        assert_eq!(
            first,
            runner_game_id("com.orivo.fixture-runner", "fixture-profile-1", "alpha")
        );
        assert_ne!(
            first,
            runner_game_id("com.orivo.fixture-runner", "fixture-profile-2", "alpha")
        );
        assert!(first.starts_with("runner:com.orivo.fixture-runner:fixture-profile-1:"));
    }
}
