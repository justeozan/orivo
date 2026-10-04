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
    Catalog, CatalogError, Game, GameSource, LaunchTarget, PluginPackageIdentity,
    RunnerGameInventoryEntry, RunnerLaunchMode, RunnerProfile, RunnerProfileStatus,
    directory_grant_key, directory_grant_slot,
};
use crate::plugin_manifest::{
    CapabilityGrant, CapabilityScope, CompatibleVersionInfo, HostCompatibility,
    MAX_PLUGIN_ID_LENGTH, PluginCapability, PluginExtension, PluginManifest,
    ValidatedPluginManifest, valid_opaque_id,
};
use crate::plugin_runtime::{
    DirectoryIdentity, PinnedDirectory, PluginDiscoveryPage, PluginGrants, PluginLaunchIntent,
    PluginLaunchMode, PluginProfileValidation, PluginRequest, PluginResponse, PluginRuntime,
    PluginRuntimeError, PreparedComponent, RunnerCheck,
};
use crate::plugin_scheduler::{JobError, JobHandle, SubmitError};
use serde::Deserialize;
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
/// What puts a hex-encoded entry name in a namespace of its own.
///
/// `plugin_runtime::valid_entry_name` never reports a name containing `:`, so no
/// identifier a plugin could have learned from a listing can begin with this —
/// which is what makes the two forms unable to describe one reference, and
/// therefore what removes any need to rank them. A `game_ref` is the key of a
/// library card (see [`runner_game_id`]), so a reference that could mean two
/// files is a card that can be silently re-pointed at the next refresh.
const HEX_REFERENCE_PREFIX: &str = "x:";
/// The longest entry name a hex reference can stand for: two digits per byte
/// inside what is left of [`MAX_EXTERNAL_ID_LENGTH`] after the prefix. Past it an
/// id names nothing the host could have listed, so decoding one is work with no
/// possible answer.
const MAX_HEX_REFERENCE_BYTES: usize = (MAX_EXTERNAL_ID_LENGTH - HEX_REFERENCE_PREFIX.len()) / 2;
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
    /// The grant was given to a package this is no longer. Permissions belong
    /// to the code the user allowed, not to the identifier it installed under.
    GrantStale,
    /// The profile was accepted by a build of the plugin that is not the one
    /// installed now, so its verdict says nothing about this one.
    ProfileNeedsRevalidation,
    /// The folder this game lives in cannot be opened right now — an external
    /// drive, most likely. Nothing is wrong with the permission and nothing
    /// else in the library is affected.
    DirectoryUnavailable,
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
    /// The profile authorises one launch shape and the plugin's intent named
    /// the other: a stream profile offered a game-file launch, or a game-file
    /// profile offered a stream. The mode is a permission the user set on the
    /// profile, so the host checks it rather than trusting either side alone.
    LaunchModeMismatch,
    /// A stream placeholder is missing its bytes, is not JSON, or is not the
    /// host's own `{"host","client","app"}` document for `moonlight`.
    StreamPlaceholder,
    /// The GameStream feed could not be reached, was refused, or could not be
    /// written. Propagated to the import as a failed profile so the message the
    /// `GameStreamFeedError` carries reaches the user.
    StreamFeed(crate::gamestream::GameStreamFeedError),
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
            Self::GrantStale => write!(
                formatter,
                "This plugin is not the package you allowed. Allow its folders again to keep using it."
            ),
            Self::ProfileNeedsRevalidation => write!(
                formatter,
                "This plugin changed since the profile was set up. Open its settings so Orivo can check the profile again."
            ),
            Self::DirectoryUnavailable => write!(
                formatter,
                "The folder this game lives in is not available right now. Reconnect it and try again; your other games are unaffected."
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
            Self::LaunchModeMismatch => write!(
                formatter,
                "This profile and its plugin disagree about how this game launches. Update the plugin, or set the profile up again."
            ),
            Self::StreamPlaceholder => write!(
                formatter,
                "This game's stream description is missing or unreadable. Import this profile again to refresh it."
            ),
            Self::StreamFeed(error) => write!(formatter, "{error}"),
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
    identity: PluginPackageIdentity,
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
            identity: PluginPackageIdentity {
                trusted: package_is_trusted(plugin_root, plugin_id, &sha256),
                fingerprint: sha256,
            },
            runtime: runtime.clone(),
            manifest,
            prepared,
        })
    }

    pub fn plugin_id(&self) -> &str {
        self.manifest.id()
    }

    /// Which code this is, and whether it arrived release-signed. Every
    /// permission the user gives is recorded against this, so a package that
    /// takes an installed identifier later does not inherit it.
    pub fn identity(&self) -> &PluginPackageIdentity {
        &self.identity
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

/// What a plugin may reach on one profile, right now.
///
/// Resolving per profile is what makes the ledger usable at all: two profiles
/// of the same plugin name their folders under the same slot — the v1 `runner`
/// world gives a component no way to declare the slots it will ask for, so it
/// hard-codes them — which is why the ledger keys a folder by profile *and*
/// slot and this translates back before the plugin sees anything.
#[derive(Debug)]
pub struct ResolvedProfileGrants {
    pub grants: PluginGrants,
    /// Slots this profile grants and the plugin may currently read.
    pub granted: BTreeSet<String>,
    /// Slots the host could not open. An unplugged drive is not a permissions
    /// problem and must not read like one, so it is reported apart from every
    /// other reason a folder might be out of reach.
    pub unavailable: BTreeSet<String>,
    /// Whether a permission was skipped because it belongs to a package this
    /// is no longer. It changes what the user is told, not what is allowed.
    pub stale: bool,
}

pub fn resolve_profile_grants(
    catalog: &Catalog,
    package: &RunnerPackage,
    profile: &RunnerProfile,
) -> Result<ResolvedProfileGrants, RunnerHostError> {
    // Which folders can be opened at all, and still lead where they led when
    // they were allowed. Whether each *is* the folder that was allowed is
    // `resolve_pinned`'s answer below, because it checks the descriptor it
    // opened rather than a path it looked up a second time.
    let mut pinned = BTreeMap::new();
    let mut unavailable = BTreeSet::new();
    for directory in &profile.game_directories {
        match open_granted_directory(directory, IdentityCheck::Skip) {
            Ok(_) => {
                pinned.insert(
                    directory.id.clone(),
                    PinnedDirectory {
                        path: directory.path.clone(),
                        identity: stored_identity(directory),
                    },
                );
            }
            Err(_) => {
                unavailable.insert(directory.id.clone());
            }
        }
    }

    let mut stale = false;
    let mut grants = Vec::new();
    let mut granted = BTreeSet::new();
    for record in catalog
        .plugin_grants
        .iter()
        .filter(|record| record.plugin_id == profile.plugin_id && record.is_active())
    {
        if !record.applies_to(package.identity()) {
            stale = true;
            continue;
        }
        let grant = record.to_capability_grant();
        match (&grant.capability, &grant.scope) {
            (PluginCapability::FilesRead, CapabilityScope::DirectoryGrants(keys)) => {
                // One folder at a time, so a folder that is no longer the one
                // the user allowed is dropped instead of failing the whole
                // resolution and taking every other game on the profile with it.
                for slot in keys
                    .iter()
                    .filter_map(|key| directory_grant_slot(key, &profile.id))
                    .filter(|slot| pinned.contains_key(*slot))
                {
                    let one = CapabilityGrant {
                        scope: CapabilityScope::DirectoryGrants(BTreeSet::from([slot.to_owned()])),
                        ..grant.clone()
                    };
                    let only = BTreeMap::from([(slot.to_owned(), pinned[slot].clone())]);
                    if PluginGrants::resolve_pinned(package.manifest(), &[one], &only).is_ok() {
                        granted.insert(slot.to_owned());
                    }
                }
                if granted.is_empty() {
                    continue;
                }
                grants.push(CapabilityGrant {
                    scope: CapabilityScope::DirectoryGrants(granted.clone()),
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
    pinned.retain(|slot, _| granted.contains(slot));
    let grants = PluginGrants::resolve_pinned(package.manifest(), &grants, &pinned)
        .map_err(|_| RunnerHostError::GrantRefused)?;
    Ok(ResolvedProfileGrants {
        grants,
        granted,
        unavailable,
        stale,
    })
}

impl ResolvedProfileGrants {
    /// The refusal that fits: a permission that belongs to another package
    /// reads differently from one the user simply has not given.
    fn missing(&self) -> RunnerHostError {
        if self.stale {
            RunnerHostError::GrantStale
        } else {
            RunnerHostError::GrantMissing
        }
    }
}

/// Whether one granted folder is currently readable by its plugin, as the
/// ledger sees it. The Plugins panel asks this to draw a folder as allowed or
/// not; the launch path asks the resolved grants instead, because by then the
/// folder has to have been opened as well as allowed.
pub fn directory_grant_is_active(
    catalog: &Catalog,
    plugin_id: &str,
    profile_id: &str,
    slot: &str,
) -> bool {
    let key = directory_grant_key(profile_id, slot);
    catalog
        .active_plugin_grant(plugin_id, PluginCapability::FilesRead)
        .is_some_and(|grant| match &grant.scope {
            CapabilityScope::DirectoryGrants(keys) => keys.contains(&key),
            _ => false,
        })
}

/// Whether the installer accepted a release signature for *these bytes*.
///
/// It used to read the marker file's path directly, for existence — which
/// answers the weaker question "is there a marker beside this plugin" and would
/// still have said yes after a component was swapped underneath one. The
/// installer's record now names the digest it was earned by and is written
/// inside the install transaction, so this is a statement about the component
/// the host is about to invoke rather than about a file next to it.
fn package_is_trusted(plugin_root: &Path, plugin_id: &str, component_sha256: &str) -> bool {
    crate::plugin_installer::component_channel(plugin_root, plugin_id, component_sha256)
        .is_official()
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
    let resolved = resolve_profile_grants(catalog, package, profile)?;
    let response = package.call(
        &resolved.grants,
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
    package: &RunnerPackage,
    profile_id: &str,
) -> Result<&'catalog RunnerProfile, RunnerHostError> {
    let profile = catalog
        .runner_profile(profile_id)
        .filter(|profile| profile.plugin_id == package.plugin_id())
        .ok_or(RunnerHostError::UnknownProfile)?;
    // A verdict is about the component that gave it. A package that changed
    // under the same identifier has never been asked about this profile, so
    // its `Valid` says nothing and the profile waits for a fresh answer rather
    // than launching on an old one.
    if profile.package_fingerprint.as_deref() != Some(package.identity().fingerprint.as_str()) {
        return Err(RunnerHostError::ProfileNeedsRevalidation);
    }
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

/// Every file in a profile's granted folders that a plugin could name at all,
/// read once.
///
/// Resolution used to walk every granted folder again for each candidate, so a
/// page of fifty cost fifty directory reads of the same folder — measured at
/// roughly five times the per-game catalogue write it was supposed to be
/// dominated by (`docs/performance.md` § 7). A page now reads its folders once
/// and answers from that.
///
/// It holds names, not paths. Whether a name is really a regular file inside the
/// granted folder is asked again, from an open descriptor, for each name a
/// reference actually matches — so the answer is about the file that is there
/// now, and a folder of four thousand entries does not cost four thousand
/// canonicalisations to look one game up.
pub struct GrantedLibrary {
    folders: Vec<GrantedFolderListing>,
}

struct GrantedFolderListing {
    grant_id: String,
    root: PathBuf,
    /// Only the names `host-files` would have reported: a plugin may name what it
    /// could have been shown, and nothing else.
    names: Vec<String>,
}

impl GrantedLibrary {
    /// Read each folder the permission still covers.
    ///
    /// A folder the permission no longer covers is not a folder to read. The
    /// launch path already refused an entry resolved in one; leaving the
    /// *resolution* free to look meant a revoked or swapped folder still decided
    /// what an import saw — and a name it happened to share could lose the game
    /// that was allowed.
    pub fn read(profile: &RunnerProfile, granted: &BTreeSet<String>) -> Self {
        let mut folders = Vec::new();
        for directory in &profile.game_directories {
            if !granted.contains(&directory.id) {
                continue;
            }
            let Ok(opened) = open_granted_directory(directory, IdentityCheck::Require) else {
                continue;
            };
            let root = opened.canonical;
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            let mut names = Vec::new();
            for entry in entries.take(MAX_DIRECTORY_SCAN).filter_map(Result::ok) {
                // `file_type` here comes from the directory entry, so a symbolic
                // link reports as one instead of as whatever it points at.
                if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str().filter(|name| listable_entry_name(name)) else {
                    continue;
                };
                names.push(name.to_owned());
            }
            folders.push(GrantedFolderListing {
                grant_id: directory.id.clone(),
                root,
                names,
            });
        }
        Self { folders }
    }

    /// Find the one file an external id names, and say which folder it was found
    /// in.
    ///
    /// The rule is the host's and it is deliberately narrow: a match is a file
    /// whose name, or whose name without its final extension, is exactly the
    /// external id, or one whose name the id spells in the hex form below.
    /// Nothing is recursive, symbolic links are not followed, and an id that
    /// matches more than once is refused rather than ranked — an ambiguous
    /// resolution is how a runner starts a different game than the one the user
    /// picked, and how a card that already exists gets re-pointed at the next
    /// refresh, since `game_ref` is that card's key (see [`runner_game_id`]).
    pub fn resolve(&self, external_id: &str) -> Result<(String, PathBuf), RunnerHostError> {
        if !valid_opaque_id(external_id, MAX_EXTERNAL_ID_LENGTH) {
            return Err(RunnerHostError::GameUnresolvable);
        }
        let decoded = decoded_entry_name(external_id);
        let mut matches = Vec::new();
        for folder in &self.folders {
            for name in &folder.names {
                let stem = name
                    .rsplit_once('.')
                    .map_or(name.as_str(), |(stem, _)| stem);
                if name != external_id
                    && stem != external_id
                    && decoded.as_deref() != Some(name.as_str())
                {
                    continue;
                }
                if let Ok(canonical) = canonical_file_inside(&folder.root.join(name), &folder.root)
                {
                    matches.push((folder.grant_id.clone(), canonical));
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
}

/// One candidate, one folder read.
///
/// Test-only: nothing in a release build resolves a single reference any more,
/// because the only production caller resolves a whole page and reads its folders
/// once for it ([`resolve_page_candidates`]). What the suites want is the rule
/// rather than the page, and saying it in one line beats building a listing to use
/// exactly once.
#[cfg(test)]
pub fn resolve_game_file(
    profile: &RunnerProfile,
    granted: &BTreeSet<String>,
    external_id: &str,
) -> Result<(String, PathBuf), RunnerHostError> {
    GrantedLibrary::read(profile, granted).resolve(external_id)
}

/// Whether the host could have told a plugin about this name at all.
///
/// The same question `host-files` answers when it lists a folder, asked here
/// because resolution must not be the weaker side of it: a plugin may name what
/// it could have been shown. A plain identifier already could not reach a hidden
/// file — the opaque grammar makes an id start with an alphanumeric — so the
/// leading dot is that rule restated for the hex form rather than a new policy.
/// It matters on any exFAT or FAT volume, where macOS writes an AppleDouble
/// `._<name>` sidecar beside every file: it carries the same extension as the
/// dump it shadows, and a four-kilobyte sidecar handed to an emulator is not a
/// launch.
fn listable_entry_name(name: &str) -> bool {
    !name.starts_with('.') && crate::plugin_runtime::valid_entry_name(name)
}

/// The entry name a hex reference stands for, or `None` for an id that is not
/// one.
///
/// This decodes; it does not authorise. What comes back is compared against the
/// names the host read out of a granted folder, never joined onto a path, so a
/// decoded `../secret` is simply a name no entry has. Three things are refused
/// outright, and each of them is a way one file could have had more than one
/// reference — which the catalogue would have turned into more than one card,
/// because it keys a card by the reference and not by the path:
///
/// * anything without the [`HEX_REFERENCE_PREFIX`] namespace;
/// * upper-case digits, so a name of *k* hex-significant bytes has one spelling
///   rather than 2^k;
/// * a name the host would never have listed ([`listable_entry_name`]).
///
/// The length bound is here so an id that is merely long cannot make the host
/// allocate.
fn decoded_entry_name(external_id: &str) -> Option<String> {
    let digits = external_id.strip_prefix(HEX_REFERENCE_PREFIX)?.as_bytes();
    if digits.is_empty()
        || !digits.len().is_multiple_of(2)
        || digits.len() / 2 > MAX_HEX_REFERENCE_BYTES
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let high = lower_hex_digit(pair[0])?;
        let low = lower_hex_digit(pair[1])?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes)
        .ok()
        .filter(|name| listable_entry_name(name))
}

/// Lower case only. `char::to_digit(16)` accepts both cases, which is what gave
/// one file 2^k references.
fn lower_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Re-check the file an inventory entry already names, immediately before it is
/// handed to a process.
///
/// Import-time resolution is not a standing authorisation: the entry may be
/// weeks old, the file may have become a symbolic link out of the folder, and
/// the grant may have been withdrawn since. All three are asked again here.
pub fn reverify_game_file(
    resolved: &ResolvedProfileGrants,
    profile: &RunnerProfile,
    entry: &RunnerGameInventoryEntry,
) -> Result<PathBuf, RunnerHostError> {
    let directory = profile
        .granted_directory(&entry.directory_grant_id)
        .ok_or(RunnerHostError::GrantMissing)?;
    // The order is the message. A folder that is simply not plugged in is not
    // a permissions problem, and telling a user to allow a folder again when
    // the drive is in a drawer sends them looking in the wrong place.
    let root = open_granted_directory(directory, IdentityCheck::Require)?;
    if !resolved.granted.contains(&entry.directory_grant_id) {
        return Err(resolved.missing());
    }
    canonical_file_inside(&entry.game_path, &root.canonical)
}

/// Whether an opened folder has to be the one a stored identity names.
///
/// The two callers want different things, and the difference matters. Resolving
/// grants only needs to know which folders can be opened at all: pinning them is
/// [`PluginGrants::resolve_pinned`]'s job, on the descriptor it opens itself, and
/// a second check here would be a second answer that could disagree with it —
/// which is the whole of the window a swapped folder needs. The launch path has
/// no such authority to defer to, because it is about to hand a path to a
/// process, so it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityCheck {
    Skip,
    Require,
}

/// A granted folder, opened.
///
/// The descriptor is held for as long as the value lives, and the identity is
/// read from it rather than from a second lookup by path: a folder swapped
/// between two path lookups is exactly how a check and the thing it checked come
/// apart.
struct OpenGrantedDirectory {
    canonical: PathBuf,
    #[cfg(unix)]
    _handle: fs::File,
}

fn open_granted_directory(
    directory: &crate::catalog::RunnerGrantedDirectory,
    check: IdentityCheck,
) -> Result<OpenGrantedDirectory, RunnerHostError> {
    #[cfg(unix)]
    {
        let handle =
            fs::File::open(&directory.path).map_err(|_| RunnerHostError::DirectoryUnavailable)?;
        let metadata = handle
            .metadata()
            .map_err(|_| RunnerHostError::DirectoryUnavailable)?;
        if !metadata.is_dir() {
            return Err(RunnerHostError::GameOutsideScope);
        }
        // Re-canonicalising catches a parent swapped for a link, which would
        // otherwise move the whole grant somewhere else while every later check
        // kept agreeing with itself.
        let canonical =
            fs::canonicalize(&directory.path).map_err(|_| RunnerHostError::DirectoryUnavailable)?;
        if canonical != directory.path {
            return Err(RunnerHostError::GameOutsideScope);
        }
        if check == IdentityCheck::Require
            && let Some(expected) = stored_identity(directory)
        {
            use std::os::unix::fs::MetadataExt;
            if DirectoryIdentity::new(metadata.dev(), metadata.ino()) != expected {
                return Err(RunnerHostError::GameOutsideScope);
            }
        }
        Ok(OpenGrantedDirectory {
            canonical,
            _handle: handle,
        })
    }
    #[cfg(not(unix))]
    {
        // Windows needs `FILE_FLAG_BACKUP_SEMANTICS` to open a directory at all,
        // and its identity needs `GetFileInformationByHandle`. Until that unsafe
        // block is written once — with the plugin read path, not twice — this is
        // the canonical-path half on its own, and a stored identity is refused
        // rather than waved through.
        let metadata =
            fs::metadata(&directory.path).map_err(|_| RunnerHostError::DirectoryUnavailable)?;
        if !metadata.is_dir() {
            return Err(RunnerHostError::GameOutsideScope);
        }
        let canonical =
            fs::canonicalize(&directory.path).map_err(|_| RunnerHostError::DirectoryUnavailable)?;
        if canonical != directory.path {
            return Err(RunnerHostError::GameOutsideScope);
        }
        if check == IdentityCheck::Require && stored_identity(directory).is_some() {
            return Err(RunnerHostError::GameOutsideScope);
        }
        Ok(OpenGrantedDirectory { canonical })
    }
}

/// What the grant recorded, in the shape the plugin host pins against.
fn stored_identity(
    directory: &crate::catalog::RunnerGrantedDirectory,
) -> Option<DirectoryIdentity> {
    Some(DirectoryIdentity::new(directory.device?, directory.inode?))
}

/// The identity to record when a folder is granted, read from a descriptor on
/// the folder rather than from its name.
pub fn directory_identity(path: &Path) -> (Option<u64>, Option<u64>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        match fs::File::open(path).and_then(|handle| handle.metadata()) {
            Ok(metadata) if metadata.is_dir() => (Some(metadata.dev()), Some(metadata.ino())),
            Ok(_) | Err(_) => (None, None),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        (None, None)
    }
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

// ---------------------------------------------------------------------------
// Stream placeholders
// ---------------------------------------------------------------------------

/// The bytes a stream placeholder may occupy. It is a host-authored document
/// of three short strings; anything larger is not the document the feed
/// writes, and reading it as one keeps a hostile file from costing anything.
const MAX_STREAM_PLACEHOLDER_BYTES: usize = 4 * 1024;
/// Bounds on what can reach the stream argument list: a host is a DNS name,
/// an IPv4 literal or an IPv6 literal with an optional port; an application
/// name is one Sunshine label. Both are also held back from a leading `-` and
/// from control characters — there is no shell anywhere in this path, but an
/// argument that looks like an option is still an argument worth refusing.
const MAX_STREAM_HOST_LENGTH: usize = 256;
const MAX_STREAM_APP_LENGTH: usize = 256;

/// The placeholder document `{"host", "client", "app"}` the GameStream feed
/// writes, one file per streamable game. The host is the only writer: a
/// plugin or a user can put bytes on disk, but those bytes still have to be
/// this document before they can start anything.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamPlaceholder {
    host: String,
    client: String,
    app: String,
}

/// Whether one value can be an argument on its own: present, bounded, not an
/// option in disguise, and free of the control characters no argv should carry.
fn argument_safe(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}

/// A stream host additionally stays inside the characters a host name is made
/// of. This is not about quoting — there is no shell — but about refusing
/// anything the host would have to second-guess later.
fn stream_host_valid(host: &str) -> bool {
    argument_safe(host, MAX_STREAM_HOST_LENGTH)
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_' | ':' | '[' | ']')
        })
}

/// Read one resolved placeholder and return the two strings the stream
/// argument list is built from. Every failure is the same refusal: the file
/// exists and is inside the granted folder (the launch checks that first), so
/// what is wrong is its content, and the user's remedy is one re-import.
fn read_stream_placeholder(path: &Path) -> Result<(String, String), RunnerHostError> {
    let bytes = read_bounded_file(path, MAX_STREAM_PLACEHOLDER_BYTES as u64)
        .map_err(|_| RunnerHostError::StreamPlaceholder)?;
    let placeholder: StreamPlaceholder =
        serde_json::from_slice(&bytes).map_err(|_| RunnerHostError::StreamPlaceholder)?;
    if placeholder.client != "moonlight" {
        return Err(RunnerHostError::StreamPlaceholder);
    }
    if !stream_host_valid(&placeholder.host)
        || !argument_safe(&placeholder.app, MAX_STREAM_APP_LENGTH)
    {
        return Err(RunnerHostError::StreamPlaceholder);
    }
    Ok((placeholder.host, placeholder.app))
}

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
    let profile = usable_profile(catalog, package, profile_id)?;
    let entry = catalog
        .runner_inventory_entry(profile_id, game_ref)
        .ok_or(RunnerHostError::InventoryMissing)?;
    let resolved = resolve_profile_grants(catalog, package, profile)?;
    // The file first, because "that drive is not plugged in" and "you took this
    // folder away" are different things to be told, and only one of them is
    // about permissions.
    let game_file = reverify_game_file(&resolved, profile, entry)?;
    // `runner.prepare` is what the user agreed to when they created the profile.
    // Without it the plugin is installed and identified and nothing more, so the
    // host does not ask it to prepare anything.
    if !resolved.grants.holds(PluginCapability::RunnerPrepare) {
        return Err(resolved.missing());
    }
    let application = resolve_application(profile)?;

    let response = package.call(
        &resolved.grants,
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

    // The launch shape is a permission the user set on the profile, not a
    // preference the plugin may exercise: the intent has to name the shape the
    // profile authorises, whichever way round the disagreement goes.
    let authorised = match profile.settings.launch_mode {
        RunnerLaunchMode::Default => PluginLaunchMode::Default,
        RunnerLaunchMode::Stream => PluginLaunchMode::Stream,
    };
    if intent.mode() != authorised {
        return Err(RunnerHostError::LaunchModeMismatch);
    }

    match intent.mode() {
        // The v1 contract's shape: the application is started in its own
        // directory with the resolved game file as its single argument.
        PluginLaunchMode::Default => Ok(PreparedRunnerLaunch {
            working_directory: application.parent().map(Path::to_path_buf),
            program: application,
            arguments: vec![game_file],
            title: entry.title.clone(),
        }),
        // A stream: the resolved file is the host's own placeholder document,
        // read and validated here, and the closed argument list is
        // `stream <host> <app>`. Nothing the plugin said reaches this list —
        // which is what makes a mode an ABI decision rather than a template.
        PluginLaunchMode::Stream => {
            let (host, app) = read_stream_placeholder(&game_file)?;
            Ok(PreparedRunnerLaunch {
                working_directory: application.parent().map(Path::to_path_buf),
                program: application,
                arguments: vec![
                    PathBuf::from("stream"),
                    PathBuf::from(host),
                    PathBuf::from(app),
                ],
                title: entry.title.clone(),
            })
        }
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
    let profile = usable_profile(&catalog, package, profile_id)?.clone();
    let resolved = resolve_profile_grants(&catalog, package, &profile)?;
    // Discovery reads the granted folder through `host-files`, so without the
    // permission in force there is nothing for the plugin to page through. A
    // profile whose only folder is unplugged says so rather than reporting a
    // permission it still has.
    if !resolved.grants.holds(PluginCapability::FilesRead) {
        if resolved.granted.is_empty() && !resolved.unavailable.is_empty() && !resolved.stale {
            return Err(RunnerHostError::DirectoryUnavailable);
        }
        return Err(resolved.missing());
    }
    let grants = &resolved.grants;

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
            grants,
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

        let imported_at = unix_millis();
        // Resolved before the lease is taken, and the profile's folders travel
        // with the result so the commit can refuse a page resolved against a
        // grant that has since changed.
        let (entries, skipped) =
            resolve_page_candidates(&profile, profile_id, &resolved.granted, &page, imported_at);
        let committed = commit_resolved_page(
            store,
            package.plugin_id(),
            profile_id,
            &profile.game_directories,
            entries,
            &page,
            imported_at,
            skipped,
        )?;
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
pub struct CommittedPage {
    pub imported: usize,
    pub refreshed: usize,
    pub skipped: usize,
}

/// One page, one transaction. The cursor moves with the candidates it describes,
/// so a crash between two pages can only ever lose the page that was in flight.
/// Turn a page of candidates into the entries the catalog would hold.
///
/// This walks granted folders, which is why it is deliberately not part of the
/// commit: a plugin that answers with fifty ids naming nothing would otherwise
/// hold the catalog's write lease for fifty directory scans, and every other
/// write in the app behind it.
///
/// Once per page, not once per candidate. Each folder is listed one time and
/// every candidate is answered from that listing, which is also what makes a page
/// internally consistent: fifty candidates used to be fifty separate readings of
/// the same folder, and the second half of a page could disagree with the first.
fn resolve_page_candidates(
    profile: &RunnerProfile,
    profile_id: &str,
    granted: &BTreeSet<String>,
    page: &PluginDiscoveryPage,
    imported_at: u64,
) -> (Vec<RunnerGameInventoryEntry>, usize) {
    let library = GrantedLibrary::read(profile, granted);
    let mut entries = Vec::with_capacity(page.games.len());
    let mut skipped = 0;
    for candidate in &page.games {
        let Ok((directory_grant_id, game_path)) = library.resolve(&candidate.external_id) else {
            skipped += 1;
            continue;
        };
        entries.push(RunnerGameInventoryEntry {
            profile_id: profile_id.to_owned(),
            game_ref: candidate.external_id.clone(),
            title: candidate.title.clone(),
            provider_id: candidate.provider_id.clone(),
            external_id: candidate.external_id.clone(),
            game_path,
            directory_grant_id,
            platform: candidate.platform.clone(),
            imported_at: Some(imported_at),
        });
    }
    (entries, skipped)
}

/// One page, one transaction, and no filesystem inside it.
///
/// The cursor moves with the candidates it describes, so a crash between two
/// pages can only ever lose the page that was in flight. The profile is read
/// again under the lease and its folders compared against the ones the page was
/// resolved against: a grant that changed while the page was being resolved
/// makes those paths stale, and a stale path is not something to write down.
pub fn commit_resolved_page(
    store: &CatalogStore,
    plugin_id: &str,
    profile_id: &str,
    resolved_against: &[crate::catalog::RunnerGrantedDirectory],
    entries: Vec<RunnerGameInventoryEntry>,
    page: &PluginDiscoveryPage,
    imported_at: u64,
    skipped: usize,
) -> Result<CommittedPage, RunnerHostError> {
    store.commit(move |catalog| {
        let profile = catalog
            .runner_profile(profile_id)
            .filter(|profile| profile.plugin_id == plugin_id)
            .cloned()
            .ok_or_else(|| {
                CatalogError::Invalid("the runner profile changed during the import".into())
            })?;
        if profile.game_directories != resolved_against {
            return Err(CatalogError::Invalid(
                "the runner profile's folders changed during the import".into(),
            ));
        }
        let mut committed = CommittedPage {
            skipped,
            ..CommittedPage::default()
        };
        for entry in entries {
            // A candidate the catalog refuses is dropped from this page, not
            // allowed to take the page with it: one unusable ROM must not stop a
            // library from importing. The undo is exact rather than a clone of
            // the whole catalog, which a page of fifty would pay for fifty
            // times while holding the write lease.
            let game = runner_catalog_game(plugin_id, profile_id, &entry);
            let previous = catalog
                .runner_inventory_entry(profile_id, &entry.game_ref)
                .cloned();
            let Ok(inserted) = catalog.upsert_runner_inventory(entry.clone()) else {
                committed.skipped += 1;
                continue;
            };
            // One game, one card. A game this library already holds — installed
            // here, or owned on a store — is the same game to the person
            // playing it, so what an import of a second way to start it adds is
            // that way, not a second entry with its own artwork, its own play
            // time and its own place in every shelf. A standalone card this
            // runner wrote under that name before is folded in at the same
            // time, so a library that already has the duplicate heals on the
            // next import rather than needing anything removed by hand.
            let host = catalog.host_game_for_title(&entry.title, &game.id);
            let written = match &host {
                Some(host_id) => {
                    let target = game.launch_target.clone();
                    let standalone = game.id.clone();
                    catalog
                        .attach_alternate_launch(host_id, target)
                        .map(|_| {
                            catalog.games.retain(|held| held.id != standalone);
                        })
                        .is_ok()
                }
                None => catalog.upsert_runner(game).is_ok(),
            };
            if !written {
                match previous {
                    Some(previous) => {
                        let _ = catalog.upsert_runner_inventory(previous);
                    }
                    None => catalog.runner_inventory.retain(|held| {
                        held.profile_id != entry.profile_id || held.game_ref != entry.game_ref
                    }),
                }
                committed.skipped += 1;
                continue;
            }
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
        alternate_launch_targets: Vec::new(),
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

    /// Every slot this profile records. A test that is not about permissions
    /// says so by allowing all of them.
    fn all_slots(profile: &RunnerProfile) -> BTreeSet<String> {
        profile
            .game_directories
            .iter()
            .map(|directory| directory.id.clone())
            .collect()
    }

    fn profile_over(root: &Path) -> RunnerProfile {
        RunnerProfile {
            id: "fixture-profile-1".into(),
            plugin_id: "com.orivo.fixture-runner".into(),
            display_name: "Fixture".into(),
            application: root.join("Emulator"),
            // Canonical, as the grant command stores it: every later check
            // compares against it, and a temporary directory on macOS lives
            // behind a link that would otherwise make the two disagree.
            game_directories: vec![RunnerGrantedDirectory {
                id: "fixture-games".into(),
                path: fs::canonicalize(root.join("games")).unwrap_or_else(|_| root.join("games")),
                device: None,
                inode: None,
            }],
            settings: RunnerProfileSettings::default(),
            status: RunnerProfileStatus::Valid,
            status_message: None,
            enabled: true,
            import_cursor: None,
            import_complete: false,
            last_imported_at: None,
            package_fingerprint: None,
        }
    }

    #[test]
    fn resolves_a_candidate_by_its_name_or_by_its_name_without_an_extension() {
        let root = temporary_root("resolve");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::write(root.join("games/alpha.rom"), b"Alpha").unwrap();
        fs::write(root.join("games/beta"), b"Beta").unwrap();
        let profile = profile_over(&root);

        let (grant, path) = resolve_game_file(&profile, &all_slots(&profile), "alpha").unwrap();
        assert_eq!(grant, "fixture-games");
        assert_eq!(path.file_name().unwrap(), "alpha.rom");
        assert_eq!(
            resolve_game_file(&profile, &all_slots(&profile), "beta")
                .unwrap()
                .1
                .file_name(),
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
            resolve_game_file(
                &profile_over(&root),
                &all_slots(&profile_over(&root)),
                "alpha",
            ),
            Err(RunnerHostError::GameUnresolvable)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The grammar an external id has to pass allows `[A-Za-z0-9._\-:]`, and a
    /// Switch dump is conventionally called `Title [0100…][v0].nsp`. Without the
    /// hex form there is no id a plugin could return for that file at all, so
    /// the first official runner would import nothing out of a normally named
    /// library.
    #[test]
    fn resolves_a_candidate_by_the_hex_encoding_of_a_name_the_grammar_forbids() {
        let root = temporary_root("hex-name");
        fs::create_dir_all(root.join("games")).unwrap();
        let name = "Super Mario Odyssey [0100000000010000][v0].nsp";
        fs::write(root.join("games").join(name), b"not a real dump").unwrap();
        let profile = profile_over(&root);

        // The plugin cannot spell this name; it can only spell its bytes.
        assert!(!valid_opaque_id(name, MAX_EXTERNAL_ID_LENGTH));
        let (grant, path) =
            resolve_game_file(&profile, &all_slots(&profile), &hex_of(name)).unwrap();
        assert_eq!(grant, "fixture-games");
        assert_eq!(path.file_name().unwrap(), name);

        fs::remove_dir_all(root).unwrap();
    }

    /// A hex reference lives in its own namespace, so a file *called* the hex of
    /// another one's name cannot collide with it. This is the first half of why
    /// there is no winner to pick: the two forms cannot describe one id.
    ///
    /// A `game_ref` is the key of a library card (`runner_game_id`), so silently
    /// re-pointing one is not a resolution detail — the next refresh would
    /// overwrite `game_path` on a card the user already has.
    #[test]
    fn a_file_named_like_a_hex_reference_cannot_be_reached_by_one() {
        let root = temporary_root("hex-collision");
        fs::create_dir_all(root.join("games")).unwrap();
        // "ab" hex-encodes to "6162" under the old, unprefixed scheme, and
        // "6162" is also a perfectly legal file name.
        fs::write(root.join("games/ab"), b"the hex-addressed one").unwrap();
        fs::write(root.join("games/6162"), b"the literally named one").unwrap();
        let profile = profile_over(&root);

        // The plain id still names the file that is literally called that.
        let (_, path) = resolve_game_file(&profile, &all_slots(&profile), "6162").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"the literally named one");
        // And the hex id names the other one, with no ambiguity between them:
        // `valid_entry_name` never lists a name containing `:`, so no plain id
        // learned from a listing can ever start with `x:`.
        let (_, path) = resolve_game_file(&profile, &all_slots(&profile), &hex_of("ab")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"the hex-addressed one");

        fs::remove_dir_all(root).unwrap();
    }

    /// The second half: more than one match is refused, whichever form found
    /// them. There used to be a precedence rule here, and precedence is how a
    /// card gets re-pointed without anyone being told.
    #[test]
    fn two_files_answering_one_reference_are_refused_rather_than_ranked() {
        let root = temporary_root("hex-ambiguous");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::create_dir_all(root.join("more")).unwrap();
        // The same name in two granted folders of one profile: the hex form
        // describes both, and neither is the obvious answer.
        fs::write(root.join("games/Zelda [0100F2C0115B6000].xci"), b"one").unwrap();
        fs::write(root.join("more/Zelda [0100F2C0115B6000].xci"), b"two").unwrap();
        let mut profile = profile_over(&root);
        profile.game_directories.push(RunnerGrantedDirectory {
            id: "second".into(),
            path: fs::canonicalize(root.join("more")).unwrap(),
            device: None,
            inode: None,
        });

        assert_eq!(
            resolve_game_file(
                &profile,
                &all_slots(&profile),
                &hex_of("Zelda [0100F2C0115B6000].xci"),
            ),
            Err(RunnerHostError::GameUnresolvable)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// One file, one reference. `to_digit(16)` used to accept upper-case, so a
    /// name of *k* hex-significant bytes had 2^k references — and the catalogue
    /// keys a card by the reference, not by the path, so each variant would have
    /// become its own card for the same file.
    #[test]
    fn only_the_lower_case_hex_form_of_a_name_resolves() {
        let root = temporary_root("hex-case");
        fs::create_dir_all(root.join("games")).unwrap();
        fs::write(root.join("games/Zelda [0100F2C0115B6000].xci"), b"one").unwrap();
        let profile = profile_over(&root);
        let canonical = hex_of("Zelda [0100F2C0115B6000].xci");

        assert!(resolve_game_file(&profile, &all_slots(&profile), &canonical).is_ok());
        let shouted = format!(
            "{}{}",
            HEX_REFERENCE_PREFIX,
            canonical[HEX_REFERENCE_PREFIX.len()..].to_ascii_uppercase()
        );
        assert_ne!(shouted, canonical);
        assert_eq!(
            resolve_game_file(&profile, &all_slots(&profile), &shouted),
            Err(RunnerHostError::GameUnresolvable)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A plugin may only name what the host could have shown it. A plain id
    /// could never name a hidden file — the opaque grammar makes an id start with
    /// an alphanumeric — and the hex form must not be the way around that:
    /// macOS writes an AppleDouble `._<name>` sidecar beside every file on an
    /// exFAT volume, and handing a four-kilobyte sidecar to an emulator is not a
    /// launch.
    #[test]
    fn a_reference_cannot_name_a_file_the_listing_would_never_have_shown() {
        let root = temporary_root("hex-unlistable");
        fs::create_dir_all(root.join("games")).unwrap();
        for name in [
            "._Super Mario Odyssey [0100000000010000][v0].nsp",
            ".hidden.nsp",
            "with:a:colon.nsp",
            "CON",
        ] {
            fs::write(root.join("games").join(name), b"never a game").unwrap();
        }
        let profile = profile_over(&root);

        for name in [
            "._Super Mario Odyssey [0100000000010000][v0].nsp",
            ".hidden.nsp",
            "with:a:colon.nsp",
            "CON",
        ] {
            assert_eq!(
                resolve_game_file(&profile, &all_slots(&profile), &hex_of(name)),
                Err(RunnerHostError::GameUnresolvable),
                "{name} is not a name host-files would list"
            );
        }
        // And the plain form, which is where this actually changed behaviour. A
        // hidden name was never reachable that way — the opaque grammar makes an
        // id start with an alphanumeric — but `with:a:colon.nsp` and `CON` both
        // pass that grammar and used to resolve by exact name, even though no
        // listing could ever have offered either.
        for name in ["with:a:colon.nsp", "with:a:colon", "CON"] {
            assert!(valid_opaque_id(name, MAX_EXTERNAL_ID_LENGTH));
            assert_eq!(
                resolve_game_file(&profile, &all_slots(&profile), name),
                Err(RunnerHostError::GameUnresolvable),
                "{name} is not a name host-files would list"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    fn hex_of(value: &str) -> String {
        let digits: String = value
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{HEX_REFERENCE_PREFIX}{digits}")
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
                resolve_game_file(&profile, &all_slots(&profile), external_id),
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
            resolve_game_file(&profile, &all_slots(&profile), "escape"),
            Err(RunnerHostError::GameUnresolvable)
        );
        // And the launch half, where the entry already exists and the file was
        // swapped underneath it. The end-to-end version of this — through a
        // real profile, its grants and a real launch — lives in
        // `runner_commands`; what is being pinned here is the check itself.
        assert_eq!(
            canonical_file_inside(&root.join("games/escape.rom"), &root.join("games")),
            Err(RunnerHostError::GameOutsideScope)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The folder the user pointed at, and not one that took its name. Both
    /// halves of the check earn their place: the canonical path catches a
    /// parent swapped for a link, the identity catches a rename with no link
    /// in it at all.
    #[cfg(unix)]
    #[test]
    fn a_granted_folder_is_recognised_by_its_own_identity() {
        let root = temporary_root("folder-identity");
        let games = root.join("games");
        fs::create_dir_all(&games).unwrap();
        let (device, inode) = directory_identity(&games);
        assert!(device.is_some() && inode.is_some());
        let granted = crate::catalog::RunnerGrantedDirectory {
            id: "fixture-games".into(),
            path: fs::canonicalize(&games).unwrap(),
            device,
            inode,
        };
        assert_eq!(
            open_granted_directory(&granted, IdentityCheck::Require)
                .unwrap()
                .canonical,
            granted.path
        );

        fs::rename(&games, root.join("games-real")).unwrap();
        fs::create_dir_all(&games).unwrap();
        assert_eq!(
            open_granted_directory(&granted, IdentityCheck::Require)
                .err()
                .expect("a folder that took the name is not the folder"),
            RunnerHostError::GameOutsideScope
        );

        fs::remove_dir_all(&games).unwrap();
        assert_eq!(
            open_granted_directory(&granted, IdentityCheck::Require)
                .err()
                .expect("a folder that is gone cannot be opened"),
            RunnerHostError::DirectoryUnavailable
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
