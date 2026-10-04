//! The commands the "Add an emulator" surface will call, and the service behind
//! them.
//!
//! Everything here obeys the same boundary as the rest of the backend: the
//! WebView sends opaque ids and display text, and never a path. A folder or an
//! application enters Orivo only through a native picker this module opens, is
//! canonicalised here, and is stored where only Rust can read it. What goes back
//! is a view model of ids, labels and states.
//!
//! The service is deliberately usable without Tauri. Every command is a thin
//! wrapper over a method on [`ThirdPartyRunnerService`], so the tests exercise
//! the code the commands run rather than an approximation of it.

use crate::catalog::{
    Catalog, CatalogError, RunnerGrantedDirectory, RunnerLaunchMode, RunnerProfile,
    RunnerProfileSettings, RunnerProfileStatus,
};
use crate::plugin_manifest::{HostCompatibility, PluginCapability, valid_opaque_id};
use crate::plugin_registry::{PluginRegistry, PluginState};
use crate::plugin_runtime::PluginRuntime;
use crate::runner_host::{
    CatalogStore, RunnerHostError, RunnerImportLimits, RunnerImportProgress, RunnerPackage,
    apply_profile_validation, directory_grant_is_active, directory_identity, import_runner_games,
    prepare_runner_launch, resolve_application, unix_millis, validate_profile_with_plugin,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::State;

/// The grant slot a folder is filed under when the caller names none.
///
/// It should come from the package: a component hard-codes the grant ids it
/// asks `host-files` for, and the v1 manifest has no field to declare them in.
/// Until it does, the host records the slot it granted under so the two sides
/// can still be made to agree, and the reference fixture's own slot is passed in
/// explicitly by the tests that use it.
pub const DEFAULT_DIRECTORY_SLOT: &str = "games";
/// Short enough that `<profile id>:<slot>` still fits the grant-scope grammar,
/// and longer than any name a component would hard-code.
const MAX_DIRECTORY_SLOT_LENGTH: usize = 96;
const MAX_PROFILE_NAME_LENGTH: usize = 120;
/// Enough concurrent imports for a user working through several runners, and
/// few enough that the map is not a way to make Orivo hold memory.
const MAX_IMPORT_JOBS: usize = 12;

// ---------------------------------------------------------------------------
// View models
// ---------------------------------------------------------------------------

/// The only third-party runner data safe to cross the IPC boundary. No
/// filesystem path, component hash or grant scope appears in it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRunnerView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: PluginState,
    pub message: String,
    pub profiles: Vec<RunnerProfileView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunnerProfileView {
    pub id: String,
    pub plugin_id: String,
    pub display_name: String,
    pub status: RunnerProfileStatus,
    pub status_message: Option<String>,
    pub enabled: bool,
    /// The application's own name, never where it lives.
    pub application_label: String,
    /// Which launch shape this profile authorises. It is the user's own
    /// permission, so the panel both shows it and sets it; the plugin is never
    /// asked about it and can never widen it.
    pub launch_mode: RunnerLaunchMode,
    pub directories: Vec<RunnerDirectoryView>,
    pub game_count: usize,
    pub import_complete: bool,
    pub import_resumable: bool,
    pub last_imported_at: Option<u64>,
}

/// What asking to pair produced. No credential is in either arm, because
/// pairing does not use one.
///
/// `AlreadyPaired` is an answer, not an error: it is the state the user wants
/// to be in, and the previous version of this call could not tell it apart from
/// success — it showed a PIN for a handshake the client had refused to start,
/// which is a PIN that can only fail on the other machine.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum GameStreamPairing {
    AlreadyPaired { host: String },
    Started { host: String, pin: String },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunnerDirectoryView {
    pub id: String,
    /// The folder's own name, never its path.
    pub label: String,
    /// Whether the plugin may currently read it. A revoked folder stays listed
    /// with its games intact; only this turns false.
    pub granted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerImportPhase {
    Running,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunnerImportJobView {
    pub job_id: String,
    pub profile_id: String,
    pub phase: RunnerImportPhase,
    pub imported: usize,
    pub refreshed: usize,
    pub skipped: usize,
    pub pages: usize,
    /// Whether this run picked up a cursor a previous one left behind.
    pub resumed: bool,
    pub complete: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RunnerImportJob {
    profile_id: String,
    cancelled: Arc<AtomicBool>,
    state: Mutex<RunnerImportState>,
}

#[derive(Debug, Clone)]
struct RunnerImportState {
    phase: RunnerImportPhase,
    progress: RunnerImportProgress,
    resumed: bool,
    complete: bool,
    message: String,
}

/// Owns everything about third-party runners that outlives one command: where
/// plugins are installed, how the catalog is written, and which imports are in
/// flight.
pub struct ThirdPartyRunnerService {
    store: CatalogStore,
    plugin_root: PathBuf,
    compatibility: HostCompatibility,
    jobs: Mutex<BTreeMap<String, Arc<RunnerImportJob>>>,
    sequence: AtomicU64,
    import_limits: RunnerImportLimits,
    /// Unset in production: one engine per process is what makes the memory
    /// ceiling and the compilation cache global. A test sets its own so its
    /// invocations do not share a bounded queue with every other test.
    runtime: Option<PluginRuntime>,
    /// The GameStream host settings, for the import that refreshes `.stream`
    /// placeholders into a stream profile's granted folder. Present in
    /// production; absent in tests that do not touch stream profiles.
    gamestream: Option<Arc<crate::gamestream::GameStreamService>>,
}

impl ThirdPartyRunnerService {
    pub fn new(
        store: CatalogStore,
        plugin_root: PathBuf,
        compatibility: HostCompatibility,
    ) -> Self {
        Self {
            store,
            plugin_root,
            compatibility,
            jobs: Mutex::new(BTreeMap::new()),
            sequence: AtomicU64::new(0),
            import_limits: RunnerImportLimits::default(),
            runtime: None,
            gamestream: None,
        }
    }

    /// Attach the GameStream service. Called by `setup` in production; by tests
    /// that exercise a stream-profile import path.
    pub fn with_gamestream(
        mut self,
        gamestream: Arc<crate::gamestream::GameStreamService>,
    ) -> Self {
        self.gamestream = Some(gamestream);
        self
    }

    /// Shrink the pages an import walks. A test that wants to cancel between two
    /// pages needs there to be two pages.
    #[cfg(test)]
    pub fn with_import_limits(mut self, limits: RunnerImportLimits) -> Self {
        self.import_limits = limits;
        self
    }

    #[cfg(test)]
    pub fn with_runtime(mut self, runtime: PluginRuntime) -> Self {
        self.runtime = Some(runtime);
        self
    }

    fn runtime(&self) -> Result<PluginRuntime, RunnerHostError> {
        // Every path that loads a runner package comes through here, and every one
        // of them is something the user asked for: listing runners, creating a
        // profile, granting a folder, importing, launching. None of them runs at
        // startup, which is why this is where the compile cache is allowed to
        // open — see `plugin_compile_cache::permit`.
        crate::plugin_compile_cache::permit();
        match self.runtime.as_ref() {
            Some(runtime) => Ok(runtime.clone()),
            None => PluginRuntime::shared().map_err(|_| RunnerHostError::RuntimeUnavailable),
        }
    }

    /// Load and verify one installed runner package. Compilation is cached by
    /// the shared engine, so re-reading the component on each operation buys a
    /// fresh hash check rather than a fresh compile.
    pub fn package(&self, plugin_id: &str) -> Result<RunnerPackage, RunnerHostError> {
        RunnerPackage::load(
            &self.runtime()?,
            &self.plugin_root,
            plugin_id,
            self.compatibility,
        )
    }

    /// Every installed runner plugin, with the profiles the user built for it.
    /// A plugin that is installed but unusable still appears, with the reason,
    /// because that is the row the user has to act on.
    pub fn installed_runners(&self) -> Result<Vec<InstalledRunnerView>, RunnerHostError> {
        let runtime = self.runtime()?;
        let catalog = self.store.snapshot()?;
        let registry = PluginRegistry::new(self.plugin_root.clone(), self.compatibility);
        Ok(registry
            .runner_plugins(&runtime)
            .into_iter()
            .map(|plugin| InstalledRunnerView {
                profiles: profile_views(&catalog, &plugin.id),
                id: plugin.id,
                name: plugin.name,
                version: plugin.version,
                state: plugin.state,
                message: plugin.message,
            })
            .collect())
    }

    pub fn profile_view(&self, profile_id: &str) -> Result<RunnerProfileView, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .ok_or(RunnerHostError::UnknownProfile)?;
        Ok(profile_view(&catalog, profile))
    }

    /// Create a profile and immediately ask its plugin what it thinks of it.
    ///
    /// The profile is persisted either way. A plugin's refusal is a state to
    /// show and correct, not a reason to throw away the application the user
    /// just picked, and a profile that was never accepted simply cannot launch.
    pub fn create_profile(
        &self,
        plugin_id: &str,
        display_name: &str,
        application: &Path,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        self.create_profile_with_id(
            &self.next_id("runner"),
            plugin_id,
            display_name,
            application,
        )
    }

    pub fn create_profile_with_id(
        &self,
        profile_id: &str,
        plugin_id: &str,
        display_name: &str,
        application: &Path,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let package = self.package(plugin_id)?;
        let display_name = display_text(display_name, MAX_PROFILE_NAME_LENGTH)
            .ok_or(RunnerHostError::UnknownProfile)?;
        let application = std::fs::canonicalize(application)
            .map_err(|_| RunnerHostError::ApplicationUnavailable)?;
        let profile = RunnerProfile {
            id: profile_id.to_owned(),
            plugin_id: plugin_id.to_owned(),
            display_name,
            application,
            game_directories: Vec::new(),
            settings: RunnerProfileSettings::default(),
            status: RunnerProfileStatus::Unvalidated,
            status_message: None,
            enabled: true,
            import_cursor: None,
            import_complete: false,
            last_imported_at: None,
            package_fingerprint: None,
        };
        // The application has to be startable before the plugin is asked about
        // anything: a profile pointing at a folder or a text file is the host's
        // to refuse, and refusing it here keeps a bad pick from becoming a
        // stored profile the plugin blessed.
        resolve_application(&profile)?;
        let identity = package.identity().clone();
        let granted_at = unix_millis();
        self.store.commit(|catalog| {
            catalog.upsert_runner_profile(profile)?;
            // Creating a profile is the consent for this plugin to prepare
            // *this* profile's launches, and for nothing else: the value is the
            // profile id, so another profile's permission is a separate answer.
            catalog.allow_plugin_scope_value(
                plugin_id,
                PluginCapability::RunnerPrepare,
                profile_id,
                &identity,
                granted_at,
            )
        })?;
        self.revalidate_profile(&package, profile_id)
    }

    pub fn rename_profile(
        &self,
        profile_id: &str,
        display_name: &str,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let display_name = display_text(display_name, MAX_PROFILE_NAME_LENGTH)
            .ok_or(RunnerHostError::UnknownProfile)?;
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .ok_or(RunnerHostError::UnknownProfile)?;
        let package = self.package(&profile.plugin_id)?;
        let renamed = RunnerProfile {
            display_name,
            ..profile.clone()
        };
        self.commit_profile(renamed)?;
        // The display name is half of the WIT `runner-profile` record, so a
        // rename is a new question for the plugin rather than a cosmetic edit.
        self.revalidate_profile(&package, profile_id)
    }

    pub fn set_profile_enabled(
        &self,
        profile_id: &str,
        enabled: bool,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .ok_or(RunnerHostError::UnknownProfile)?;
        self.commit_profile(RunnerProfile {
            enabled,
            ..profile.clone()
        })?;
        self.profile_view(profile_id)
    }

    /// Set which launch shape this profile authorises.
    ///
    /// The plugin is deliberately not asked again: the mode is the user's own
    /// permission over their own machine, it is not part of the WIT
    /// `runner-profile` record `validate-profile` sees, and a plugin that
    /// could influence it would be choosing its own argument list. The import
    /// cursor is reset instead, because the folder's meaning changed — a
    /// stream profile's folder is a feed Orivo refreshes, a default one's is
    /// games the user put there — and the next import should walk it whole
    /// rather than resume a cursor earned under the other reading.
    ///
    /// Games already imported keep their cards. One whose launch shape no
    /// longer matches refuses with [`RunnerHostError::LaunchModeMismatch`],
    /// which says so and names the remedy, rather than being deleted here
    /// behind the user's back.
    pub fn set_profile_launch_mode(
        &self,
        profile_id: &str,
        launch_mode: RunnerLaunchMode,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .ok_or(RunnerHostError::UnknownProfile)?;
        if profile.settings.launch_mode == launch_mode {
            return self.profile_view(profile_id);
        }
        self.commit_profile(RunnerProfile {
            settings: RunnerProfileSettings { launch_mode },
            import_cursor: None,
            import_complete: false,
            ..profile.clone()
        })?;
        self.profile_view(profile_id)
    }

    pub fn delete_profile(&self, profile_id: &str) -> Result<bool, RunnerHostError> {
        let revoked_at = unix_millis();
        self.cancel_jobs_for_profile(profile_id);
        self.store
            .commit(|catalog| catalog.remove_runner_profile(profile_id, revoked_at))
    }

    /// Record a folder on a profile and put the permission to read it in force.
    ///
    /// Both halves land in one transaction because they answer one question. A
    /// folder on the profile with no grant behind it would be a scope nothing
    /// authorises; a grant naming a folder no profile records would be a
    /// permission over nothing.
    pub fn grant_directory(
        &self,
        profile_id: &str,
        slot: Option<&str>,
        directory: &Path,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let slot = slot.unwrap_or(DEFAULT_DIRECTORY_SLOT).to_owned();
        // `:` is what joins a profile id to a slot in the ledger, so a slot
        // carrying one could spell another profile's key. The opaque grammar
        // allows it; a slot does not.
        if !valid_opaque_id(&slot, MAX_DIRECTORY_SLOT_LENGTH) || slot.contains(':') {
            return Err(RunnerHostError::GrantRefused);
        }
        let directory =
            std::fs::canonicalize(directory).map_err(|_| RunnerHostError::GameOutsideScope)?;
        if !directory.is_dir() {
            return Err(RunnerHostError::GameOutsideScope);
        }
        let plugin_id = {
            let catalog = self.store.snapshot()?;
            catalog
                .runner_profile(profile_id)
                .map(|profile| profile.plugin_id.clone())
                .ok_or(RunnerHostError::UnknownProfile)?
        };
        // The permission is recorded against the package that is installed now,
        // so a different one arriving under this id later does not inherit it.
        let identity = self.package(&plugin_id)?.identity().clone();
        // Taken here, while the folder is the one the picker returned.
        let (device, inode) = directory_identity(&directory);
        let granted_at = unix_millis();
        self.store.commit(|catalog| {
            let profile = catalog
                .runner_profile(profile_id)
                .cloned()
                .ok_or_else(|| CatalogError::Invalid("unknown runner profile".into()))?;
            // Re-pointing a slot that games were already imported under would
            // strand those games outside their own grant. Saying so is clearer
            // than letting whole-catalog validation refuse the write.
            if profile
                .granted_directory(&slot)
                .is_some_and(|existing| existing.path != directory)
                && catalog
                    .runner_inventory
                    .iter()
                    .any(|entry| entry.profile_id == profile_id && entry.directory_grant_id == slot)
            {
                return Err(CatalogError::Invalid(
                    "this folder slot already holds imported games".into(),
                ));
            }
            let mut directories = profile
                .game_directories
                .iter()
                .filter(|existing| existing.id != slot)
                .cloned()
                .collect::<Vec<_>>();
            directories.push(RunnerGrantedDirectory {
                id: slot.clone(),
                path: directory.clone(),
                device,
                inode,
            });
            catalog.upsert_runner_profile(RunnerProfile {
                game_directories: directories,
                ..profile.clone()
            })?;
            // One folder, one value, keyed to this profile. Restating a whole
            // scope here is what used to put a revoked folder back the next
            // time the user allowed a different one.
            catalog.allow_plugin_scope_value(
                &profile.plugin_id,
                PluginCapability::FilesRead,
                &crate::catalog::directory_grant_key(profile_id, &slot),
                &identity,
                granted_at,
            )
        })?;
        self.profile_view(profile_id)
    }

    /// Withdraw the permission to read one folder. The folder stays on the
    /// profile and every game imported from it stays in the library: this takes
    /// a permission away and nothing else.
    pub fn revoke_directory(
        &self,
        profile_id: &str,
        directory_id: &str,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let revoked_at = unix_millis();
        self.store.commit(|catalog| {
            catalog.revoke_runner_directory(profile_id, directory_id, revoked_at)
        })?;
        self.profile_view(profile_id)
    }

    /// Ask the plugin about the profile as it stands now and store the verdict.
    fn revalidate_profile(
        &self,
        package: &RunnerPackage,
        profile_id: &str,
    ) -> Result<RunnerProfileView, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .cloned()
            .ok_or(RunnerHostError::UnknownProfile)?;
        let cancelled = AtomicBool::new(false);
        let validation = validate_profile_with_plugin(package, &catalog, &profile, &cancelled)?;
        let mut validated = profile;
        apply_profile_validation(&mut validated, &validation);
        // The verdict belongs to the component that gave it, so the two are
        // written together and read together.
        validated.package_fingerprint = Some(package.identity().fingerprint.clone());
        self.commit_profile(validated)?;
        self.profile_view(profile_id)
    }

    /// Persist a profile, and touch no permission doing it.
    ///
    /// Renaming, enabling and revalidating say nothing about folders. Restating
    /// a scope from whatever the catalog happens to hold is how a folder the
    /// user revoked came back the next time they edited anything, so a profile
    /// write is only ever a profile write.
    fn commit_profile(&self, profile: RunnerProfile) -> Result<(), RunnerHostError> {
        self.store
            .commit(|catalog| catalog.upsert_runner_profile(profile).map(|_| ()))
    }

    /// Everything a plugin's removal costs it.
    ///
    /// Every permission it held is taken back, and every profile it owns goes
    /// back to waiting for a verdict — a package that arrives under this id
    /// later has not been asked about any of them. What stays is what belongs
    /// to the user: the profiles they built, the folders they picked and every
    /// game already imported, which is the plan's promise 6 for a plugin that
    /// is disabled or deleted.
    ///
    /// Called from `lib.rs`, on the installer's identity-change seam: on an
    /// uninstall, and on any change of package that breaks the consent chain —
    /// a different signer, an unsigned build, or a rollback to an older one.
    /// The package identity recorded on each grant is the second line of
    /// defence, for a package that changes without the installer being the one
    /// that changed it.
    pub fn forget_plugin(&self, plugin_id: &str) -> Result<(), RunnerHostError> {
        let revoked_at = unix_millis();
        let plugin_id = plugin_id.to_owned();
        self.store.commit(move |catalog| {
            catalog.revoke_plugin_grants(&plugin_id, revoked_at)?;
            for profile in catalog
                .runner_profiles_for_plugin(&plugin_id)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                catalog.upsert_runner_profile(RunnerProfile {
                    status: RunnerProfileStatus::Unvalidated,
                    status_message: None,
                    package_fingerprint: None,
                    ..profile
                })?;
            }
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // Imports
    // -----------------------------------------------------------------------

    /// Run one import to completion on the calling thread. `start_import` is
    /// this, on a worker, with a job to watch it by.
    pub fn import_now(
        &self,
        profile_id: &str,
        cancelled: &AtomicBool,
        mut on_progress: impl FnMut(RunnerImportProgress),
    ) -> Result<crate::runner_host::RunnerImportOutcome, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .ok_or(RunnerHostError::UnknownProfile)?;
        let package = self.package(&profile.plugin_id)?;
        // A stream profile's games are fetched by the host and written as
        // `.stream` placeholders before the plugin walks them. The refresh is
        // skipped when no host is configured — that is the manual mode the
        // plugin documents — and surfaces any other feed error as a failed
        // import so the message the feed carries reaches the user.
        if profile.settings.launch_mode == RunnerLaunchMode::Stream {
            // The *first* folder the profile still has access to, and only it.
            // One host's library written into two folders would be the same
            // game twice, so a second folder on a stream profile is somewhere
            // the user maintains themselves rather than a second feed.
            let destination = profile
                .game_directories
                .iter()
                .find(|directory| {
                    directory_grant_is_active(
                        &catalog,
                        &profile.plugin_id,
                        profile_id,
                        &directory.id,
                    )
                })
                .map(|directory| directory.path.clone());
            // The client that answers is the profile's own application, and it
            // is re-resolved and re-checked here exactly as a launch would:
            // asking it is as much "running it" as starting a game is.
            let client = resolve_application(profile)?;
            if let (Some(destination), Some(service)) = (destination, self.gamestream.as_ref()) {
                match service.refresh_placeholders(&client, &destination) {
                    // Manual placeholders remain and are imported as-is.
                    Err(crate::gamestream::GameStreamFeedError::NotConfigured) | Ok(_) => {}
                    Err(error) => return Err(RunnerHostError::StreamFeed(error)),
                }
            }
        }
        import_runner_games(
            &package,
            &self.store,
            profile_id,
            self.import_limits,
            cancelled,
            &mut on_progress,
        )
    }

    pub fn start_import(self: &Arc<Self>, profile_id: &str) -> Result<String, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        if catalog.runner_profile(profile_id).is_none() {
            return Err(RunnerHostError::UnknownProfile);
        }
        let job_id = self.next_id("runner-import");
        let job = Arc::new(RunnerImportJob {
            profile_id: profile_id.to_owned(),
            cancelled: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(RunnerImportState {
                phase: RunnerImportPhase::Running,
                progress: RunnerImportProgress::default(),
                resumed: false,
                complete: false,
                message: "Looking for games…".into(),
            }),
        });
        {
            let mut jobs = self.lock_jobs()?;
            // Finished jobs are only kept so the panel that started one can read
            // its result. Dropping the oldest keeps that from being a leak.
            while jobs.len() >= MAX_IMPORT_JOBS {
                let Some(oldest) = jobs
                    .iter()
                    .find(|(_, job)| !job.is_running())
                    .map(|(id, _)| id.clone())
                else {
                    return Err(RunnerHostError::Busy);
                };
                jobs.remove(&oldest);
            }
            jobs.insert(job_id.clone(), Arc::clone(&job));
        }

        let service = Arc::clone(self);
        let worker_job = Arc::clone(&job);
        let profile = profile_id.to_owned();
        // An import is a job, not a command: the WebView gets its id back
        // immediately and watches it, so navigation never waits on a plugin.
        thread::Builder::new()
            .name("orivo-runner-import".into())
            .spawn(move || {
                let progress_job = Arc::clone(&worker_job);
                let outcome = service.import_now(&profile, &worker_job.cancelled, |progress| {
                    progress_job.report(progress);
                });
                worker_job.finish(outcome);
            })
            .map_err(|_| RunnerHostError::RuntimeUnavailable)?;
        self.import_status(&job_id)
            .map(|view| view.job_id)
            .or(Ok(job_id))
    }

    pub fn import_status(&self, job_id: &str) -> Result<RunnerImportJobView, RunnerHostError> {
        let job = self
            .lock_jobs()?
            .get(job_id)
            .cloned()
            .ok_or(RunnerHostError::UnknownProfile)?;
        Ok(job.view(job_id))
    }

    pub fn cancel_import(&self, job_id: &str) -> Result<RunnerImportJobView, RunnerHostError> {
        let job = self
            .lock_jobs()?
            .get(job_id)
            .cloned()
            .ok_or(RunnerHostError::UnknownProfile)?;
        job.cancelled.store(true, Ordering::Release);
        Ok(job.view(job_id))
    }

    fn cancel_jobs_for_profile(&self, profile_id: &str) {
        let Ok(jobs) = self.lock_jobs() else {
            return;
        };
        for job in jobs.values() {
            if job.profile_id == profile_id {
                job.cancelled.store(true, Ordering::Release);
            }
        }
    }

    fn lock_jobs(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, Arc<RunnerImportJob>>>, RunnerHostError>
    {
        self.jobs
            .lock()
            .map_err(|_| RunnerHostError::RuntimeUnavailable)
    }

    // -----------------------------------------------------------------------
    // Launch
    // -----------------------------------------------------------------------

    /// Resolve and start one third-party runner game. Returns the title the
    /// plugin's inventory recorded, which is what the player sees start.
    pub fn launch(
        &self,
        plugin_id: &str,
        profile_id: &str,
        game_ref: &str,
    ) -> Result<String, RunnerHostError> {
        let package = self.package(plugin_id)?;
        let catalog = self.store.snapshot()?;
        let cancelled = AtomicBool::new(false);
        let prepared = prepare_runner_launch(&package, &catalog, profile_id, game_ref, &cancelled)?;
        prepared.spawn()?;
        Ok(prepared.title().to_owned())
    }

    /// Start pairing this profile's client with the configured machine, and
    /// return the PIN the user has to type on the machine's own side.
    ///
    /// Pairing is not the feed's business: it is the client's own handshake
    /// with the machine, on the machine's own ports, and it is what has to
    /// happen once before anything can be listed or streamed. Nothing on this
    /// path reads a credential, because there is none to read.
    ///
    /// **The client chooses the PIN**, which is why Orivo can show it: the
    /// handshake has the client commit to a PIN and the machine's operator
    /// confirm the same one. Orivo draws it, hands it to the client as an
    /// argument, and the user types it into Sunshine or Apollo.
    ///
    /// The state is checked first, and that check is the whole point of this
    /// shape: a client that is already paired *refuses to start a handshake*,
    /// so starting one anyway and showing its PIN would hand the user four
    /// digits that the other machine can only reject. Already paired is
    /// therefore an answer, not an attempt.
    ///
    /// Once started, the process is left running on purpose. `pair` blocks
    /// until the machine confirms or the attempt times out, so returning as
    /// soon as it has started is the only way to show the PIN while it is
    /// still worth typing; a thread reaps the child so repeated attempts do
    /// not pile up.
    pub fn begin_pairing(&self, profile_id: &str) -> Result<GameStreamPairing, RunnerHostError> {
        let catalog = self.store.snapshot()?;
        let profile = catalog
            .runner_profile(profile_id)
            .ok_or(RunnerHostError::UnknownProfile)?;
        // Pairing a profile that does not launch streams would be pairing on
        // behalf of a permission the user never gave.
        if profile.settings.launch_mode != RunnerLaunchMode::Stream {
            return Err(RunnerHostError::LaunchModeMismatch);
        }
        // Re-resolved and re-checked here, exactly as a launch would: storing a
        // path was never an authorisation to run it.
        let application = resolve_application(profile)?;
        let gamestream = self.gamestream.as_ref().ok_or(RunnerHostError::StreamFeed(
            crate::gamestream::GameStreamFeedError::NotConfigured,
        ))?;
        let host = gamestream
            .stream_host()
            .map_err(RunnerHostError::StreamFeed)?;
        if gamestream
            .is_paired(&application)
            .map_err(RunnerHostError::StreamFeed)?
        {
            return Ok(GameStreamPairing::AlreadyPaired { host });
        }
        let pin = pairing_pin()?;
        let child = pairing_command(&application, &host, &pin)
            .spawn()
            .map_err(|_| RunnerHostError::ApplicationUnavailable)?;
        thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        Ok(GameStreamPairing::Started { host, pin })
    }

    /// The transactional writer, for tests that have to set up a catalog state
    /// no command produces — a planted inventory entry a hostile plugin would
    /// have produced, for one.
    #[cfg(test)]
    pub fn store_for_tests(&self) -> &CatalogStore {
        &self.store
    }

    fn next_id(&self, kind: &str) -> String {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let mut digest = Sha256::new();
        digest.update(kind.as_bytes());
        digest.update(nanos.to_le_bytes());
        digest.update(sequence.to_le_bytes());
        digest.update(std::process::id().to_le_bytes());
        format!("{kind}-{}", &format!("{:x}", digest.finalize())[..24])
    }
}

impl RunnerImportJob {
    fn is_running(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.phase == RunnerImportPhase::Running)
            .unwrap_or(false)
    }

    fn report(&self, progress: RunnerImportProgress) {
        if let Ok(mut state) = self.state.lock() {
            state.progress = progress;
            state.message = format!(
                "Imported {} game(s) from {} page(s).",
                progress.imported + progress.refreshed,
                progress.pages
            );
        }
    }

    fn finish(&self, outcome: Result<crate::runner_host::RunnerImportOutcome, RunnerHostError>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match outcome {
            Ok(outcome) => {
                state.progress = outcome.progress;
                state.resumed = outcome.resumed_from.is_some();
                state.complete = outcome.complete;
                state.phase = if outcome.cancelled {
                    RunnerImportPhase::Cancelled
                } else {
                    RunnerImportPhase::Ready
                };
                state.message = if outcome.cancelled {
                    "Import stopped. Orivo kept its place and will continue from there.".into()
                } else {
                    format!(
                        "Imported {} game(s), refreshed {}.",
                        outcome.progress.imported, outcome.progress.refreshed
                    )
                };
            }
            Err(error) => {
                state.phase = RunnerImportPhase::Failed;
                state.message = error.to_string();
            }
        }
    }

    fn view(&self, job_id: &str) -> RunnerImportJobView {
        let state = self.state.lock().ok().map(|state| state.clone());
        let state = state.unwrap_or(RunnerImportState {
            phase: RunnerImportPhase::Failed,
            progress: RunnerImportProgress::default(),
            resumed: false,
            complete: false,
            message: "This import is no longer available.".into(),
        });
        RunnerImportJobView {
            job_id: job_id.to_owned(),
            profile_id: self.profile_id.clone(),
            phase: state.phase,
            imported: state.progress.imported,
            refreshed: state.progress.refreshed,
            skipped: state.progress.skipped,
            pages: state.progress.pages,
            resumed: state.resumed,
            complete: state.complete,
            message: state.message,
        }
    }
}

fn profile_views(catalog: &Catalog, plugin_id: &str) -> Vec<RunnerProfileView> {
    catalog
        .runner_profiles_for_plugin(plugin_id)
        .into_iter()
        .map(|profile| profile_view(catalog, profile))
        .collect()
}

fn profile_view(catalog: &Catalog, profile: &RunnerProfile) -> RunnerProfileView {
    RunnerProfileView {
        id: profile.id.clone(),
        plugin_id: profile.plugin_id.clone(),
        display_name: profile.display_name.clone(),
        status: profile.status,
        status_message: profile.status_message.clone(),
        enabled: profile.enabled,
        application_label: safe_label(&profile.application, "Emulator"),
        launch_mode: profile.settings.launch_mode,
        directories: profile
            .game_directories
            .iter()
            .map(|directory| RunnerDirectoryView {
                granted: directory_grant_is_active(
                    catalog,
                    &profile.plugin_id,
                    &profile.id,
                    &directory.id,
                ),
                label: safe_label(&directory.path, "Folder"),
                id: directory.id.clone(),
            })
            .collect(),
        game_count: catalog
            .runner_inventory
            .iter()
            .filter(|entry| entry.profile_id == profile.id)
            .count(),
        import_complete: profile.import_complete,
        import_resumable: !profile.import_complete && profile.import_cursor.is_some(),
        last_imported_at: profile.last_imported_at,
    }
}

/// A name, never a location. The same rule the Wine surface follows: a label is
/// the last component, trimmed, bounded and stripped of control characters.
fn safe_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .map(|name| name.chars().take(96).collect())
        .unwrap_or_else(|| fallback.into())
}

fn display_text(value: &str, max_length: usize) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed.len() <= max_length && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.to_owned())
}

fn opaque(value: &str) -> bool {
    valid_opaque_id(value, 256)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

type Service<'state> = State<'state, Arc<ThirdPartyRunnerService>>;

#[tauri::command]
pub async fn get_installed_runners(
    service: Service<'_>,
) -> Result<Vec<InstalledRunnerView>, String> {
    let service = Arc::clone(&service);
    // Discovery compiles and probes third-party components, so it belongs on a
    // blocking worker rather than the command executor.
    tauri::async_runtime::spawn_blocking(move || service.installed_runners())
        .await
        .map_err(|_| "Orivo could not read your installed runners.".to_string())?
        .map_err(|error| error.to_string())
}

/// Create a profile for an installed runner. The emulation application is
/// chosen here, in Rust, through the operating system's own picker: the WebView
/// supplies a plugin id and a name and nothing that could be a path.
#[tauri::command]
pub async fn create_runner_profile(
    plugin_id: String,
    display_name: String,
    client_id: Option<String>,
    service: Service<'_>,
) -> Result<RunnerProfileView, String> {
    if !opaque(&plugin_id) {
        return Err("That runner plugin is not installed.".into());
    }
    if client_id.as_deref().is_some_and(|id| !opaque(id)) {
        return Err("That application is no longer available.".into());
    }
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || {
        // A handle for something Orivo found itself, or the native picker. The
        // WebView never names a path either way: an id is only honoured if it
        // still matches a client detection finds on this machine now, so the
        // set of programs it can choose from is the set Orivo looks for.
        let application = match client_id {
            Some(id) => crate::gamestream::client_for_id(&id)
                .ok_or_else(|| "That application is no longer available.".to_string())?,
            None => pick_application()?,
        };
        service
            .create_profile(&plugin_id, &display_name, &application)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "Setting up this runner did not finish. Try again.".to_string())?
}

/// The streaming clients Orivo can see on this machine, so a stream profile can
/// be set up in one click instead of a file picker. Labels and handles only.
#[tauri::command]
pub async fn find_stream_clients() -> Result<Vec<crate::gamestream::DetectedClientView>, String> {
    // Touching the filesystem, so not on the command executor.
    tauri::async_runtime::spawn_blocking(|| {
        crate::gamestream::detect_clients()
            .iter()
            .map(crate::gamestream::DetectedClient::view)
            .collect()
    })
    .await
    .map_err(|_| "Orivo could not look for a streaming client.".to_string())
}

#[tauri::command]
pub async fn rename_runner_profile(
    profile_id: String,
    display_name: String,
    service: Service<'_>,
) -> Result<RunnerProfileView, String> {
    if !opaque(&profile_id) {
        return Err("This runner profile is no longer available.".into());
    }
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.rename_profile(&profile_id, &display_name))
        .await
        .map_err(|_| "Renaming this runner profile did not finish. Try again.".to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_runner_profile_enabled(
    profile_id: String,
    enabled: bool,
    service: Service<'_>,
) -> Result<RunnerProfileView, String> {
    if !opaque(&profile_id) {
        return Err("This runner profile is no longer available.".into());
    }
    service
        .set_profile_enabled(&profile_id, enabled)
        .map_err(|error| error.to_string())
}

/// Switch a profile between the launch shapes the host implements. The value
/// is matched against a closed set here: an unknown mode is refused rather
/// than defaulted, so a WebView cannot name a shape this host does not build.
#[tauri::command]
pub fn set_runner_profile_launch_mode(
    profile_id: String,
    launch_mode: String,
    service: Service<'_>,
) -> Result<RunnerProfileView, String> {
    if !opaque(&profile_id) {
        return Err("This runner profile is no longer available.".into());
    }
    let launch_mode = match launch_mode.as_str() {
        "default" => RunnerLaunchMode::Default,
        "stream" => RunnerLaunchMode::Stream,
        _ => return Err("Orivo does not know that way of launching a game.".into()),
    };
    service
        .set_profile_launch_mode(&profile_id, launch_mode)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_runner_profile(profile_id: String, service: Service<'_>) -> Result<bool, String> {
    if !opaque(&profile_id) {
        return Err("This runner profile is no longer available.".into());
    }
    service
        .delete_profile(&profile_id)
        .map_err(|error| error.to_string())
}

/// The pairing process, built the way every other process in this host is: no
/// shell, one argument each, and every value one Orivo produced itself — the
/// program is a canonical path it resolved, the address passed the feed's own
/// host grammar, and the PIN is four digits it drew. Nothing a plugin said is
/// anywhere in it, because a plugin is not involved in pairing at all.
fn pairing_command(application: &Path, host: &str, pin: &str) -> Command {
    let mut command = Command::new(application);
    command
        .arg("pair")
        .arg(host)
        .arg("--pin")
        .arg(pin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// A uniform four-digit pairing PIN.
///
/// Rejection-sampled rather than reduced modulo 10000: 65536 is not a multiple
/// of it, so a plain `%` would make the first 5536 PINs slightly likelier than
/// the rest. A pairing PIN is a short-lived shared secret for a handshake that
/// authorises a client against a machine, and skewing it costs nothing to
/// avoid.
fn pairing_pin() -> Result<String, RunnerHostError> {
    loop {
        let mut bytes = [0u8; 2];
        getrandom::fill(&mut bytes).map_err(|_| RunnerHostError::ApplicationUnavailable)?;
        let sample = u16::from_le_bytes(bytes);
        if sample < 60_000 {
            return Ok(format!("{:04}", sample % 10_000));
        }
    }
}

/// Start pairing a stream profile's client with the configured host. The PIN
/// that comes back is for the user to type on the host's own side; Orivo never
/// sends it anywhere itself.
#[tauri::command]
pub async fn begin_gamestream_pairing(
    profile_id: String,
    service: Service<'_>,
) -> Result<GameStreamPairing, String> {
    if !opaque(&profile_id) {
        return Err("This runner profile is no longer available.".into());
    }
    let service = Arc::clone(&service);
    // Resolving and starting the client is blocking work, so it belongs on a
    // worker rather than the command executor.
    tauri::async_runtime::spawn_blocking(move || service.begin_pairing(&profile_id))
        .await
        .map_err(|_| "Pairing did not start. Try again.".to_string())?
        .map_err(|error| error.to_string())
}

/// Allow one folder for a profile. Like the application, the folder comes from
/// a native picker opened by Rust; `slot` is the opaque grant id the plugin will
/// name it by, never a path.
#[tauri::command]
pub async fn grant_runner_profile_directory(
    profile_id: String,
    slot: Option<String>,
    service: Service<'_>,
) -> Result<RunnerProfileView, String> {
    if !opaque(&profile_id) || slot.as_deref().is_some_and(|slot| !opaque(slot)) {
        return Err("This runner profile is no longer available.".into());
    }
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || {
        let directory = pick_directory()?;
        service
            .grant_directory(&profile_id, slot.as_deref(), &directory)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "Allowing that folder did not finish. Try again.".to_string())?
}

#[tauri::command]
pub fn revoke_runner_profile_directory(
    profile_id: String,
    directory_id: String,
    service: Service<'_>,
) -> Result<RunnerProfileView, String> {
    if !opaque(&profile_id) || !opaque(&directory_id) {
        return Err("This runner profile is no longer available.".into());
    }
    service
        .revoke_directory(&profile_id, &directory_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn start_runner_import(
    profile_id: String,
    service: Service<'_>,
) -> Result<RunnerImportJobView, String> {
    if !opaque(&profile_id) {
        return Err("This runner profile is no longer available.".into());
    }
    let service = Arc::clone(&service);
    let job_id = service
        .start_import(&profile_id)
        .map_err(|error| error.to_string())?;
    service
        .import_status(&job_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_runner_import_status(
    job_id: String,
    service: Service<'_>,
) -> Result<RunnerImportJobView, String> {
    if !opaque(&job_id) {
        return Err("This import is no longer available.".into());
    }
    service
        .import_status(&job_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_runner_import(
    job_id: String,
    service: Service<'_>,
) -> Result<RunnerImportJobView, String> {
    if !opaque(&job_id) {
        return Err("This import is no longer available.".into());
    }
    service
        .cancel_import(&job_id)
        .map_err(|error| error.to_string())
}

#[cfg(desktop)]
fn pick_application() -> Result<PathBuf, String> {
    rfd::FileDialog::new()
        .set_title("Choose the emulator application for this runner")
        .pick_file()
        .ok_or_else(|| "No emulator was chosen.".to_string())
}

#[cfg(mobile)]
fn pick_application() -> Result<PathBuf, String> {
    Err("Third-party runners cannot be configured on this device yet.".into())
}

#[cfg(desktop)]
fn pick_directory() -> Result<PathBuf, String> {
    rfd::FileDialog::new()
        .set_title("Choose a games folder for this runner")
        .pick_folder()
        .ok_or_else(|| "No folder was chosen.".to_string())
}

#[cfg(mobile)]
fn pick_directory() -> Result<PathBuf, String> {
    Err("Third-party runners cannot be configured on this device yet.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{LaunchTarget, RunnerLaunchMode};
    use crate::plugin_manifest::{
        ArtifactDescriptor, ArtifactKind, PLUGIN_SDK_V1, PluginCapability, PluginExtension,
        PluginManifest,
    };
    use crate::plugin_runtime::{EpochMode, PluginLimits};
    use crate::plugin_update::PackageChannel;
    use crate::runner_host::{RunnerHostError, resolve_game_file};
    use std::{
        fs,
        sync::{RwLock, atomic::AtomicU64},
    };

    /// The reference third-party runner from `src-tauri/fixtures`. These tests
    /// are end to end on purpose: the plugin really is a WebAssembly component
    /// the host compiles, verifies and calls, because everything interesting
    /// here is about what happens between its answer and a process.
    const FIXTURE: &[u8] = include_bytes!("../fixtures/orivo-runner-fixture.wasm");
    const FIXTURE_PLUGIN_ID: &str = "com.orivo.fixture-runner";
    /// The fixture only accepts a profile whose id begins with `fixture`, which
    /// is how a test gets both an accepted and a refused profile out of one
    /// component.
    const ACCEPTED_PROFILE_ID: &str = "fixture-profile-1";
    /// The grant slot the fixture component asks `host-files` for. It is
    /// hard-coded in the component because the v1 manifest has nowhere to
    /// declare it.
    const FIXTURE_SLOT: &str = "fixture-games";
    const SECOND_PROFILE_ID: &str = "fixture-profile-2";

    struct Harness {
        root: PathBuf,
        plugin_root: PathBuf,
        games: PathBuf,
        catalog_path: PathBuf,
        emulator: PathBuf,
        service: Arc<ThirdPartyRunnerService>,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    impl Harness {
        fn new(tag: &str, roms: &[(&str, &str)]) -> Self {
            Self::with_limits(tag, roms, RunnerImportLimits::default())
        }

        fn with_limits(tag: &str, roms: &[(&str, &str)], limits: RunnerImportLimits) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "orivo-third-party-runner-{tag}-{}-{}-{}",
                std::process::id(),
                unix_millis(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let plugin_root = root.join("plugins");
            let games = root.join("games");
            fs::create_dir_all(&games).unwrap();
            fs::write(root.join("secret.txt"), b"a keychain token").unwrap();
            for (name, title) in roms {
                fs::write(games.join(name), title.as_bytes()).unwrap();
            }
            write_fixture_plugin(&plugin_root);
            let emulator = fake_emulator(&root);
            let catalog_path = root.join("catalog.json");
            let service = Self::service(&plugin_root, &catalog_path, Catalog::default(), limits);
            Self {
                root,
                plugin_root,
                games,
                catalog_path,
                emulator,
                service,
            }
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
                .with_runtime(
                    PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Threaded)
                        .unwrap(),
                )
                .with_import_limits(limits),
            )
        }

        /// A second service over the same file, as a restart is: nothing in
        /// memory carries over, and whatever the import persisted is all there
        /// is to resume from.
        fn restart(&self, limits: RunnerImportLimits) -> Arc<ThirdPartyRunnerService> {
            let catalog = Catalog::load(&self.catalog_path).unwrap();
            Self::service(&self.plugin_root, &self.catalog_path, catalog, limits)
        }

        /// The full flow the "Add an emulator" surface will drive: a profile the
        /// plugin accepted, one allowed folder, and the grants both imply.
        fn configured_profile(&self) -> RunnerProfileView {
            self.service
                .create_profile_with_id(
                    ACCEPTED_PROFILE_ID,
                    FIXTURE_PLUGIN_ID,
                    "Fixture Runner",
                    &self.emulator,
                )
                .unwrap();
            self.service
                .grant_directory(ACCEPTED_PROFILE_ID, Some(FIXTURE_SLOT), &self.games)
                .unwrap()
        }

        fn import(&self) -> crate::runner_host::RunnerImportOutcome {
            let cancelled = AtomicBool::new(false);
            self.service
                .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
                .unwrap()
        }

        fn catalog(&self) -> Catalog {
            Catalog::load(&self.catalog_path).unwrap()
        }

        /// A second profile of the same plugin, with its own folder under the
        /// same slot. That is not a contrived shape: the component hard-codes
        /// the slot it asks for, so every profile it owns uses the same one.
        fn second_profile(&self, tag: &str, roms: &[(&str, &str)]) -> PathBuf {
            let folder = self.root.join(tag);
            fs::create_dir_all(&folder).unwrap();
            for (name, title) in roms {
                fs::write(folder.join(name), title.as_bytes()).unwrap();
            }
            self.service
                .create_profile_with_id(
                    SECOND_PROFILE_ID,
                    FIXTURE_PLUGIN_ID,
                    "Second Fixture",
                    &self.emulator,
                )
                .unwrap();
            self.service
                .grant_directory(SECOND_PROFILE_ID, Some(FIXTURE_SLOT), &folder)
                .unwrap();
            let cancelled = AtomicBool::new(false);
            self.service
                .import_now(SECOND_PROFILE_ID, &cancelled, |_| {})
                .unwrap();
            folder
        }

        /// The other half of the junction: the installer, over the same plugin
        /// root, with the observer `lib.rs` registers wired to this service.
        ///
        /// It is the real wiring rather than a stand-in — the store announces
        /// that a package changed, `grant_verdict` decides whether the consent
        /// chain survived it, and `forget_plugin` is what records that it did
        /// not. A junction tested with a hand-written callback would prove the
        /// callback works and nothing about what `lib.rs` does.
        /// Returns the announcement counter beside it: "the package did not
        /// change" and "the package changed and the chain held" are different
        /// facts, and a test that only looks at the ledger cannot tell them
        /// apart.
        fn installer(
            &self,
        ) -> (
            crate::plugin_installer::PluginInstallerService,
            Arc<AtomicU64>,
        ) {
            let installer = crate::plugin_installer::PluginInstallerService::new(
                self.plugin_root.clone(),
                env!("CARGO_PKG_VERSION"),
            );
            let announced = Arc::new(AtomicU64::new(0));
            let counter = Arc::clone(&announced);
            installer.observe_identity(Arc::new(move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
            }));
            let runners = Arc::clone(&self.service);
            installer.observe_identity(Arc::new(move |change| {
                let lapsed = match change {
                    crate::plugin_update::IdentityChange::Removed { .. } => true,
                    crate::plugin_update::IdentityChange::Activated { previous, current } => {
                        crate::plugin_update::grant_verdict(previous.as_ref(), current)
                            == crate::plugin_update::GrantVerdict::Revalidate
                    }
                };
                if lapsed {
                    let _ = runners.forget_plugin(change.plugin_id());
                }
            }));
            (installer, announced)
        }

        /// Install the fixture through the real transaction, under a channel of
        /// the caller's choosing. The package the harness writes by hand is the
        /// same one, so what changes between calls is only how it arrived.
        fn install(
            &self,
            installer: &crate::plugin_installer::PluginInstallerService,
            channel: crate::plugin_update::PackageChannel,
        ) -> Result<(), String> {
            let files = crate::plugin_installer::read_package(&fixture_package())?;
            crate::plugin_installer::install_verified(
                installer,
                FIXTURE_PLUGIN_ID,
                "1.0.0",
                channel,
                &files,
                false,
            )
            .map(|_| ())
        }

        fn grants_active(&self) -> usize {
            self.catalog()
                .plugin_grants
                .iter()
                .filter(|grant| grant.is_active())
                .count()
        }

        /// The marker the installer writes beside a package it accepted with a
        /// release signature. Removing it is what a hand-loaded package taking
        /// over an installed id looks like from here.
        fn trust_marker(&self) -> PathBuf {
            self.plugin_root
                .join(".staging")
                .join("trusted")
                .join(FIXTURE_PLUGIN_ID)
        }
    }

    /// The fixture as a `.orivo-plugin` archive, so the junction tests can put
    /// it through the installer's real transaction instead of writing the tree
    /// by hand the way `write_fixture_plugin` does.
    fn fixture_package() -> Vec<u8> {
        use flate2::{Compression, write::GzEncoder};
        let files: [(&str, Vec<u8>); 2] = [
            (
                "manifest.json",
                serde_json::to_vec(&fixture_manifest()).unwrap(),
            ),
            ("component.wasm", FIXTURE.to_vec()),
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

    fn fixture_component_sha256() -> String {
        let mut digest = Sha256::new();
        digest.update(FIXTURE);
        format!("{:x}", digest.finalize())
    }

    fn fixture_manifest() -> PluginManifest {
        let component_sha256 = fixture_component_sha256();
        PluginManifest {
            id: FIXTURE_PLUGIN_ID.into(),
            name: "Fixture Runner".into(),
            version: "1.0.0".into(),
            sdk: PLUGIN_SDK_V1.into(),
            min_orivo_version: Some("0.3.0".into()),
            extensions: vec![PluginExtension::Runner],
            capabilities: vec![PluginCapability::RunnerPrepare, PluginCapability::FilesRead],
            network_domains: Vec::new(),
            artifacts: vec![ArtifactDescriptor {
                path: "component.wasm".into(),
                kind: ArtifactKind::Component,
                sha256: component_sha256,
                byte_size: FIXTURE.len() as u64,
            }],
        }
    }

    fn write_fixture_plugin(plugin_root: &Path) {
        let directory = plugin_root.join(FIXTURE_PLUGIN_ID);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("component.wasm"), FIXTURE).unwrap();
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&fixture_manifest()).unwrap(),
        )
        .unwrap();
        let component_sha256 = fixture_component_sha256();
        // Installed from the signed registry channel, which is the state whose
        // grants must not carry over to a package that arrives another way. The
        // record names the component it was earned by — the format is
        // `plugin_update`'s, written inside the install transaction — so this
        // fixture has to stand in for a real install rather than for a file
        // that merely exists.
        let trusted = plugin_root.join(".staging").join("trusted");
        fs::create_dir_all(&trusted).unwrap();
        fs::write(
            trusted.join(FIXTURE_PLUGIN_ID),
            serde_json::json!({
                "signer": "orivo-release-v1",
                "componentSha256": component_sha256,
            })
            .to_string(),
        )
        .unwrap();
    }

    /// A fake emulation application: this test binary, copied.
    ///
    /// It is a real executable rather than a shell script on purpose — the host
    /// starts a process with no interpreter in between, so a script would test
    /// something else. Started with a path as its only argument, libtest reads
    /// that as a filter, matches no test and exits successfully, which is
    /// exactly the "it really ran" signal a launch test needs.
    fn fake_emulator(root: &Path) -> PathBuf {
        let emulator = root.join("Fixture Emulator");
        fs::copy(std::env::current_exe().unwrap(), &emulator).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&emulator, fs::Permissions::from_mode(0o755)).unwrap();
        }
        emulator
    }

    fn launch_error(service: &ThirdPartyRunnerService, game_ref: &str) -> RunnerHostError {
        profile_launch_error(service, ACCEPTED_PROFILE_ID, game_ref)
    }

    fn profile_launch_error(
        service: &ThirdPartyRunnerService,
        profile_id: &str,
        game_ref: &str,
    ) -> RunnerHostError {
        service
            .launch(FIXTURE_PLUGIN_ID, profile_id, game_ref)
            .expect_err("the launch should have been refused")
    }

    /// Write an inventory entry by hand, so a launch test can point a game
    /// reference the plugin misbehaves on at a file that really exists. A
    /// hostile plugin reaches the same state by returning that reference from
    /// `discover-page`; going through the catalog keeps the test's filenames
    /// portable.
    fn plant_entry(harness: &Harness, game_ref: &str, file: &str) {
        let catalog = harness.catalog();
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap();
        let (directory_grant_id, game_path) = resolve_game_file(
            profile,
            &profile
                .game_directories
                .iter()
                .map(|directory| directory.id.clone())
                .collect(),
            file,
        )
        .unwrap();
        let entry = crate::catalog::RunnerGameInventoryEntry {
            profile_id: ACCEPTED_PROFILE_ID.into(),
            game_ref: game_ref.into(),
            title: "Planted Game".into(),
            provider_id: FIXTURE_PLUGIN_ID.into(),
            external_id: game_ref.into(),
            game_path,
            directory_grant_id,
            platform: None,
            imported_at: Some(1),
        };
        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                let game = crate::runner_host::runner_catalog_game(
                    FIXTURE_PLUGIN_ID,
                    ACCEPTED_PROFILE_ID,
                    &entry,
                );
                catalog.upsert_runner_inventory(entry.clone())?;
                catalog.upsert_runner(game)?;
                Ok(())
            })
            .unwrap();
    }

    /// Plant an entry in a named folder without going through discovery. The
    /// fixture only ever pages the one slot it hard-codes, so a second folder
    /// needs its inventory written the way an import would have written it.
    fn plant_entry_in(harness: &Harness, game_ref: &str, file: &str, slot: &str, folder: &Path) {
        let entry = crate::catalog::RunnerGameInventoryEntry {
            profile_id: ACCEPTED_PROFILE_ID.into(),
            game_ref: game_ref.into(),
            title: "Planted Game".into(),
            provider_id: FIXTURE_PLUGIN_ID.into(),
            external_id: game_ref.into(),
            game_path: fs::canonicalize(folder.join(file)).unwrap(),
            directory_grant_id: slot.into(),
            platform: None,
            imported_at: Some(1),
        };
        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                let game = crate::runner_host::runner_catalog_game(
                    FIXTURE_PLUGIN_ID,
                    ACCEPTED_PROFILE_ID,
                    &entry,
                );
                catalog.upsert_runner_inventory(entry.clone())?;
                catalog.upsert_runner(game)?;
                Ok(())
            })
            .unwrap();
    }

    const THREE_ROMS: &[(&str, &str)] = &[
        ("alpha.rom", "Alpha Quest"),
        ("beta.rom", "Beta Racer"),
        ("gamma.rom", "Gamma Tactics"),
    ];

    // -----------------------------------------------------------------------
    // The whole flow
    // -----------------------------------------------------------------------

    #[test]
    fn a_configured_third_party_runner_imports_its_library_and_launches_a_game() {
        let harness = Harness::new("happy", THREE_ROMS);
        let profile = harness.configured_profile();
        assert_eq!(profile.status, RunnerProfileStatus::Valid);
        assert_eq!(profile.directories.len(), 1);
        assert!(profile.directories[0].granted);

        let outcome = harness.import();
        assert_eq!(outcome.progress.imported, 3);
        assert_eq!(outcome.progress.skipped, 0);
        assert!(outcome.complete);

        let catalog = harness.catalog();
        assert_eq!(catalog.runner_inventory.len(), 3);
        let card = catalog
            .games
            .iter()
            .find(|game| game.title == "Alpha Quest")
            .expect("the imported card");
        assert_eq!(
            card.launch_target,
            LaunchTarget::Runner {
                runner_id: FIXTURE_PLUGIN_ID.into(),
                game_ref: "alpha".into(),
                profile_id: ACCEPTED_PROFILE_ID.into(),
            }
        );
        // A runner card carries references and nothing that resembles a command.
        assert!(card.executable_path.is_none());
        assert!(card.arguments.is_empty());
        assert!(card.working_directory.is_none());

        let title = harness
            .service
            .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
            .unwrap();
        assert_eq!(title, "Alpha Quest");
    }

    /// The process the host builds is the whole security claim of this lot: one
    /// program it resolved from the profile, one argument it resolved inside a
    /// granted folder, and nothing a plugin wrote.
    #[test]
    fn the_process_is_the_profile_application_with_one_resolved_argument() {
        let harness = Harness::new("command", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        let package = harness.service.package(FIXTURE_PLUGIN_ID).unwrap();
        let cancelled = AtomicBool::new(false);
        let prepared = crate::runner_host::prepare_runner_launch(
            &package,
            &harness.catalog(),
            ACCEPTED_PROFILE_ID,
            "alpha",
            &cancelled,
        )
        .unwrap();
        let command = prepared.command();
        assert_eq!(
            command.get_program(),
            fs::canonicalize(&harness.emulator).unwrap().as_os_str()
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                fs::canonicalize(harness.games.join("alpha.rom"))
                    .unwrap()
                    .as_os_str()
            ]
        );
        assert_eq!(
            command.get_current_dir(),
            Some(fs::canonicalize(&harness.root).unwrap().as_path())
        );

        // And it really is a process: no shell, no interpreter, and the child
        // starts from the file the host resolved.
        let mut child = prepared.spawn().unwrap();
        assert!(child.wait().unwrap().success());
    }

    // -----------------------------------------------------------------------
    // Streams
    // -----------------------------------------------------------------------

    /// Turn the configured profile into one that authorises game streams,
    /// through the same call the panel makes. The mode is the user's own
    /// permission on the profile, so it changes without asking the plugin
    /// again — `validate-profile` never sees it, which is exactly why the
    /// launch checks the intent against it later.
    fn make_stream_profile(harness: &Harness) {
        harness
            .service
            .set_profile_launch_mode(ACCEPTED_PROFILE_ID, RunnerLaunchMode::Stream)
            .expect("the mode is the user's to set");
    }

    fn write_placeholder(harness: &Harness, file: &str, body: &str) {
        fs::write(harness.games.join(file), body).unwrap();
    }

    /// The mode is on the view so the panel can show it, and it round-trips
    /// through the same call the panel makes. Changing it drops the import
    /// cursor: the folder now means something else — a feed Orivo refreshes
    /// rather than games the user put there — so the next import walks it
    /// whole instead of resuming a cursor earned under the other reading.
    #[test]
    fn the_launch_mode_is_the_users_to_set_and_a_change_drops_the_import_cursor() {
        let harness = Harness::new("launch-mode", THREE_ROMS);
        harness.configured_profile();
        assert_eq!(
            harness
                .service
                .profile_view(ACCEPTED_PROFILE_ID)
                .unwrap()
                .launch_mode,
            RunnerLaunchMode::Default
        );

        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                let mut profile = catalog
                    .runner_profile(ACCEPTED_PROFILE_ID)
                    .expect("the configured profile")
                    .clone();
                profile.import_cursor = Some("page-2".into());
                catalog.upsert_runner_profile(profile)
            })
            .unwrap();
        assert!(
            harness
                .service
                .profile_view(ACCEPTED_PROFILE_ID)
                .unwrap()
                .import_resumable
        );

        let view = harness
            .service
            .set_profile_launch_mode(ACCEPTED_PROFILE_ID, RunnerLaunchMode::Stream)
            .unwrap();
        assert_eq!(view.launch_mode, RunnerLaunchMode::Stream);
        assert!(
            !view.import_resumable,
            "the cursor was earned the other way"
        );
        // And it is still the user's to take back.
        assert_eq!(
            harness
                .service
                .set_profile_launch_mode(ACCEPTED_PROFILE_ID, RunnerLaunchMode::Default)
                .unwrap()
                .launch_mode,
            RunnerLaunchMode::Default
        );
    }

    /// Setting the mode a profile already has changes nothing — not the mode,
    /// and in particular not a cursor an import is going to resume from.
    #[test]
    fn setting_the_mode_a_profile_already_has_keeps_its_import_cursor() {
        let harness = Harness::new("launch-mode-same", THREE_ROMS);
        harness.configured_profile();
        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                let mut profile = catalog
                    .runner_profile(ACCEPTED_PROFILE_ID)
                    .expect("the configured profile")
                    .clone();
                profile.import_cursor = Some("page-2".into());
                catalog.upsert_runner_profile(profile)
            })
            .unwrap();

        let view = harness
            .service
            .set_profile_launch_mode(ACCEPTED_PROFILE_ID, RunnerLaunchMode::Default)
            .unwrap();
        assert_eq!(view.launch_mode, RunnerLaunchMode::Default);
        assert!(
            view.import_resumable,
            "nothing changed, so nothing was lost"
        );
    }

    #[test]
    fn the_launch_mode_of_an_unknown_profile_cannot_be_set() {
        let harness = Harness::new("launch-mode-unknown", THREE_ROMS);
        harness.configured_profile();
        assert_eq!(
            harness
                .service
                .set_profile_launch_mode("runner:nope", RunnerLaunchMode::Stream)
                .unwrap_err(),
            RunnerHostError::UnknownProfile
        );
    }

    /// The stream launch end to end: the profile authorises streams, the
    /// placeholder beside the game is the host's own document, and the process
    /// that comes out is the closed `stream <host> <app>` argument list —
    /// every element of which the host read and validated itself.
    #[test]
    fn a_stream_profile_launches_with_the_closed_stream_argument_list() {
        let harness = Harness::new("stream", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        write_placeholder(
            &harness,
            "Modulus.stream",
            r#"{ "host": "astra.local", "client": "moonlight", "app": "Modulus" }"#,
        );
        plant_entry(&harness, "fixture:stream", "Modulus.stream");

        let package = harness.service.package(FIXTURE_PLUGIN_ID).unwrap();
        let cancelled = AtomicBool::new(false);
        let prepared = crate::runner_host::prepare_runner_launch(
            &package,
            &harness.catalog(),
            ACCEPTED_PROFILE_ID,
            "fixture:stream",
            &cancelled,
        )
        .unwrap();
        assert_eq!(
            prepared.command().get_program(),
            fs::canonicalize(&harness.emulator).unwrap().as_os_str()
        );
        assert_eq!(
            prepared
                .command()
                .get_args()
                .collect::<Vec<_>>()
                .iter()
                .map(|argument| argument.to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["stream", "astra.local", "Modulus"]
        );
        assert_eq!(prepared.title(), "Planted Game");
    }

    /// The mode on the profile is a permission, and the intent has to match it
    /// in both directions: a stream offer from a profile that only ever
    /// authorises game files is as refused as the other way round.
    #[test]
    fn a_stream_intent_on_a_game_file_profile_is_refused() {
        let harness = Harness::new("stream-mismatch", THREE_ROMS);
        harness.configured_profile();
        write_placeholder(
            &harness,
            "Modulus.stream",
            r#"{ "host": "astra.local", "client": "moonlight", "app": "Modulus" }"#,
        );
        plant_entry(&harness, "fixture:stream", "Modulus.stream");

        assert_eq!(
            launch_error(&harness.service, "fixture:stream"),
            RunnerHostError::LaunchModeMismatch
        );
    }

    #[test]
    fn a_game_file_intent_on_a_stream_profile_is_refused() {
        let harness = Harness::new("file-mismatch", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        plant_entry(&harness, "fixture:ok", "alpha.rom");

        assert_eq!(
            launch_error(&harness.service, "fixture:ok"),
            RunnerHostError::LaunchModeMismatch
        );
    }

    /// The placeholder exists and is inside the granted folder; what it may
    /// not be is anything but the host's own document — bytes that are not
    /// JSON, a client the host does not drive, or a host that looks like an
    /// option. None of them reach an argument list.
    #[test]
    fn a_stream_placeholder_that_is_not_the_documents_word_is_refused() {
        let harness = Harness::new("stream-placeholder", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        write_placeholder(&harness, "Broken.stream", "not json at all");
        write_placeholder(
            &harness,
            "Alien.stream",
            r#"{ "host": "astra.local", "client": "steamlink", "app": "Modulus" }"#,
        );
        write_placeholder(
            &harness,
            "Optioned.stream",
            r#"{ "host": "-astra.local", "client": "moonlight", "app": "Modulus" }"#,
        );
        plant_entry(&harness, "fixture:stream-broken", "Broken.stream");
        plant_entry(&harness, "fixture:stream-alien", "Alien.stream");
        plant_entry(&harness, "fixture:stream-optioned", "Optioned.stream");

        for game_ref in [
            "fixture:stream-broken",
            "fixture:stream-alien",
            "fixture:stream-optioned",
        ] {
            assert_eq!(
                launch_error(&harness.service, game_ref),
                RunnerHostError::StreamPlaceholder,
                "{game_ref}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Pairing
    // -----------------------------------------------------------------------

    /// The PIN is a short-lived shared secret for a handshake that authorises a
    /// client against a machine, so its shape is worth asserting: four digits,
    /// always, including the leading zeros a plain integer would have dropped.
    #[test]
    fn a_pairing_pin_is_always_four_digits() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let pin = pairing_pin().unwrap();
            assert_eq!(pin.len(), 4, "{pin}");
            assert!(pin.chars().all(|digit| digit.is_ascii_digit()), "{pin}");
            seen.insert(pin);
        }
        // Not a statistical test — just that it is drawn rather than fixed.
        assert!(seen.len() > 100, "{} distinct pins", seen.len());
    }

    /// Pairing is Moonlight's own handshake, so the argument list is the
    /// client's — `pair <host> --pin <pin>` — and every element of it is one
    /// Orivo produced. No plugin is consulted anywhere in this path.
    #[test]
    fn the_pairing_process_is_the_clients_own_closed_argument_list() {
        let command = pairing_command(Path::new("/Applications/Moonlight"), "astra.local", "0417");
        assert_eq!(command.get_program(), "/Applications/Moonlight");
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["pair", "astra.local", "--pin", "0417"]
        );
    }

    /// A profile that does not launch streams is not a profile to pair: doing
    /// it would be acting on a permission the user never gave.
    #[test]
    fn a_profile_that_does_not_stream_cannot_be_paired() {
        let harness = Harness::new("pair-mode", THREE_ROMS);
        harness.configured_profile();
        assert_eq!(
            harness
                .service
                .begin_pairing(ACCEPTED_PROFILE_ID)
                .unwrap_err(),
            RunnerHostError::LaunchModeMismatch
        );
    }

    /// Pairing needs the address and nothing else — so with no address it is
    /// the feed's "not configured" refusal, which already names the remedy.
    #[test]
    fn pairing_without_a_configured_host_says_what_is_missing() {
        let harness = Harness::new("pair-nohost", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        assert_eq!(
            harness
                .service
                .begin_pairing(ACCEPTED_PROFILE_ID)
                .unwrap_err(),
            RunnerHostError::StreamFeed(crate::gamestream::GameStreamFeedError::NotConfigured)
        );
    }

    /// The happy path. The address the answer carries is the machine's, not
    /// Sunshine's web interface: pairing is the machine's own protocol on its
    /// own ports, so an API port the user typed is deliberately not in it.
    ///
    /// No credential is read on this path either, which is the point — pairing
    /// is offered precisely when nothing else about the machine works yet.
    #[cfg(unix)]
    #[test]
    fn pairing_starts_the_client_against_the_machine_not_the_web_api() {
        let harness = Harness::new("pair-ok", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        // Refuses to list, which is what "not paired yet" looks like.
        use_stream_client(&harness, "", 1);
        let service = stream_service(&harness, "https://astra.local:47990");

        let answer = service.begin_pairing(ACCEPTED_PROFILE_ID).unwrap();

        let GameStreamPairing::Started { host, pin } = answer else {
            panic!("a client that cannot list has not been paired: {answer:?}");
        };
        assert_eq!(host, "astra.local");
        assert_eq!(pin.len(), 4);
        assert!(pin.chars().all(|digit| digit.is_ascii_digit()));
        // The state was read first, and only then was the handshake started.
        assert_eq!(
            client_invocations(&harness, 6),
            vec!["list", "astra.local", "pair", "astra.local", "--pin", &pin]
        );
    }

    /// The defect this shape exists to prevent: a client that is already paired
    /// *refuses to start a handshake*, so showing its PIN would hand the user
    /// four digits the other machine can only reject. Already paired is an
    /// answer, and no PIN is drawn at all.
    #[cfg(unix)]
    #[test]
    fn a_machine_that_is_already_paired_is_told_so_instead_of_given_a_pin() {
        let harness = Harness::new("pair-already", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        use_stream_client(&harness, "Modulus\n", 0);
        let service = stream_service(&harness, "astra.local");

        assert_eq!(
            service.begin_pairing(ACCEPTED_PROFILE_ID).unwrap(),
            GameStreamPairing::AlreadyPaired {
                host: "astra.local".into()
            }
        );
        // Asked once, and never told to pair.
        assert_eq!(client_invocations(&harness, 2), vec!["list", "astra.local"]);
    }

    /// A stand-in for the streaming client: a script that answers `list` the
    /// way Moonlight does and records the arguments it was given.
    ///
    /// The harness's own application cannot stand in here. It is a copy of the
    /// test binary — which is exactly right for a launch, where libtest reads
    /// the one path argument as a filter, matches nothing and exits — but run
    /// with `list` or `pair` it would read *those* as filters and run this
    /// suite again, inside itself.
    #[cfg(unix)]
    fn use_stream_client(harness: &Harness, stdout: &str, code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = harness.root.join("Fixture Client");
        let log = harness.root.join("client.args");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nprintf '%s' '{}'\nexit {code}\n",
                log.display(),
                stdout.replace('\'', "'\\''"),
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let application = fs::canonicalize(&path).unwrap();
        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                let mut profile = catalog
                    .runner_profile(ACCEPTED_PROFILE_ID)
                    .expect("the configured profile")
                    .clone();
                profile.application = application.clone();
                catalog.upsert_runner_profile(profile)
            })
            .unwrap();
        application
    }

    /// The arguments the stand-in client was called with, once there are
    /// `expected` of them.
    ///
    /// Polled rather than read once: pairing returns as soon as the child has
    /// *started*, which is the whole point of it — the PIN has to be on screen
    /// while the handshake is still open — so the child may not have written
    /// its line yet when the call comes back.
    #[cfg(unix)]
    fn client_invocations(harness: &Harness, expected: usize) -> Vec<String> {
        let deadline = SystemTime::now() + std::time::Duration::from_secs(5);
        loop {
            let lines: Vec<String> = fs::read_to_string(harness.root.join("client.args"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect();
            if lines.len() >= expected || SystemTime::now() >= deadline {
                return lines;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Build a service for `harness` with a GameStream service bolted on, so the
    /// stream-refresh path runs. The harness keeps its own service; this one is
    /// only for tests that need the feed.
    fn stream_service(harness: &Harness, host: &str) -> Arc<ThirdPartyRunnerService> {
        let catalog = harness.catalog();
        let store = crate::runner_host::CatalogStore::new(
            Arc::new(RwLock::new(catalog)),
            harness.catalog_path.clone(),
            Arc::new(Mutex::new(())),
        );
        let gamestream = Arc::new(crate::gamestream::GameStreamService::load(
            harness.root.join(crate::gamestream::SETTINGS_FILE),
        ));
        gamestream
            .update(crate::gamestream::GameStreamSettingsUpdate {
                host: Some(host.to_owned()),
            })
            .unwrap();
        Arc::new(
            ThirdPartyRunnerService::new(
                store,
                harness.plugin_root.clone(),
                HostCompatibility::v1(env!("CARGO_PKG_VERSION")),
            )
            .with_runtime(
                PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Threaded).unwrap(),
            )
            .with_gamestream(gamestream),
        )
    }

    // -----------------------------------------------------------------------
    // Stream import refreshes the host's feed into the granted folder
    // -----------------------------------------------------------------------

    /// A stream profile's import asks the client which games the machine can
    /// stream and writes one `.stream` placeholder per game into the granted
    /// folder before the plugin walks it. The import itself still has to
    /// succeed.
    #[cfg(unix)]
    #[test]
    fn a_stream_profile_import_refreshes_placeholders_before_discovery() {
        let harness = Harness::new("stream-import", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        use_stream_client(&harness, "Modulus\nDesktop\n", 0);
        let service = stream_service(&harness, "https://astra.local:47990");

        let cancelled = AtomicBool::new(false);
        service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .expect("the import succeeds after the refresh");

        // The client was asked, with the closed argument list and no credential.
        assert_eq!(client_invocations(&harness, 2), vec!["list", "astra.local"]);
        // And the placeholder the host wrote is exactly the closed-shape
        // document, naming the machine rather than its web interface.
        let placeholder: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(harness.games.join("Modulus.stream")).unwrap(),
        )
        .unwrap();
        assert_eq!(placeholder["host"], "astra.local");
        assert_eq!(placeholder["client"], "moonlight");
        assert_eq!(placeholder["app"], "Modulus");
    }

    /// No host configured is the manual mode the plugin documents, not an
    /// import failure: the refresh is a no-op and the existing folder is
    /// walked as-is.
    #[test]
    fn a_stream_profile_with_no_host_configured_keeps_the_manual_folder() {
        let harness = Harness::new("stream-manual", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        // A hand-maintained placeholder the user wrote themselves survives the
        // unconfigured refresh, because the feed is simply not consulted.
        write_placeholder(
            &harness,
            "Modulus.stream",
            r#"{ "host": "astra.local", "client": "moonlight", "app": "Modulus" }"#,
        );
        fs::write(harness.root.join(crate::gamestream::SETTINGS_FILE), b"{}").unwrap();
        let gamestream = Arc::new(crate::gamestream::GameStreamService::load(
            harness.root.join(crate::gamestream::SETTINGS_FILE),
        ));
        let catalog = harness.catalog();
        let store = crate::runner_host::CatalogStore::new(
            Arc::new(RwLock::new(catalog)),
            harness.catalog_path.clone(),
            Arc::new(Mutex::new(())),
        );
        let service = Arc::new(
            ThirdPartyRunnerService::new(
                store,
                harness.plugin_root.clone(),
                HostCompatibility::v1(env!("CARGO_PKG_VERSION")),
            )
            .with_runtime(
                PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Threaded).unwrap(),
            )
            .with_gamestream(gamestream),
        );

        let cancelled = AtomicBool::new(false);
        let outcome = service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .expect("manual mode is not a failure");
        // The import ran to the end of the folder rather than stopping at the
        // unconsulted feed, and the folder it walked is the one on disk.
        assert!(outcome.complete, "{outcome:?}");
        assert!(!outcome.cancelled, "{outcome:?}");
        assert!(outcome.progress.imported > 0, "{outcome:?}");

        let kept = fs::read_to_string(harness.games.join("Modulus.stream")).unwrap();
        assert!(kept.contains("astra.local"), "{kept}");
    }

    /// A machine that refuses surfaces as a failed import carrying the feed's
    /// own message, not as a generic catalog error. Overwhelmingly that means
    /// the client is not paired with it yet, which is the one thing the user
    /// can act on — so that is what the message names.
    #[cfg(unix)]
    #[test]
    fn a_machine_that_refuses_fails_the_stream_import() {
        let harness = Harness::new("stream-refused", THREE_ROMS);
        harness.configured_profile();
        make_stream_profile(&harness);
        use_stream_client(&harness, "", 1);
        let service = stream_service(&harness, "astra.local");

        let cancelled = AtomicBool::new(false);
        assert_eq!(
            service
                .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
                .unwrap_err(),
            RunnerHostError::StreamFeed(crate::gamestream::GameStreamFeedError::NotPaired(
                "astra.local".into()
            )),
        );
    }

    // -----------------------------------------------------------------------
    // One game, one card
    // -----------------------------------------------------------------------

    /// Put a game in the library that is not this runner's, under `title`.
    fn plant_library_game(harness: &Harness, id: &str, title: &str) {
        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                catalog.games.push(crate::catalog::Game {
                    id: id.to_owned(),
                    title: title.to_owned(),
                    executable_path: None,
                    source: crate::catalog::GameSource::Steam,
                    source_id: Some("42".into()),
                    launch_target: LaunchTarget::Steam { app_id: 42 },
                    alternate_launch_targets: Vec::new(),
                    installation_path: Some(std::path::PathBuf::from("/tmp")),
                    working_directory: None,
                    arguments: Vec::new(),
                    description: None,
                    metadata: None,
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
                    extra: Default::default(),
                });
                Ok(())
            })
            .unwrap();
    }

    fn game_titled<'a>(catalog: &'a Catalog, title: &str) -> Vec<&'a crate::catalog::Game> {
        catalog
            .games
            .iter()
            .filter(|game| game.title == title)
            .collect()
    }

    /// A game the library already holds is one game to the person playing it,
    /// so importing a second way to start it adds that way to the card that is
    /// already there — with its artwork, its play time and its place in every
    /// shelf — rather than a second entry beside it.
    ///
    /// The punctuation matters: the same game is spelled differently by every
    /// store that sells it, and `Alpha: Quest!` has to find `Alpha Quest`.
    #[test]
    fn importing_a_game_the_library_already_holds_adds_a_way_to_start_it() {
        let harness = Harness::new("dedupe-existing", THREE_ROMS);
        harness.configured_profile();
        plant_library_game(&harness, "steam:42", "Alpha: Quest!");

        let cancelled = AtomicBool::new(false);
        harness
            .service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .unwrap();

        let catalog = harness.catalog();
        // One card for that game, and it is still the one that was there.
        assert_eq!(game_titled(&catalog, "Alpha: Quest!").len(), 1);
        assert!(game_titled(&catalog, "Alpha Quest").is_empty());
        let host = catalog
            .games
            .iter()
            .find(|game| game.id == "steam:42")
            .unwrap();
        assert_eq!(host.alternate_launch_targets.len(), 1);
        assert!(matches!(
            &host.alternate_launch_targets[0],
            LaunchTarget::Runner { runner_id, profile_id, .. }
                if runner_id == FIXTURE_PLUGIN_ID && profile_id == ACCEPTED_PROFILE_ID
        ));
        // The two games the library did not have are cards of their own.
        assert_eq!(game_titled(&catalog, "Beta Racer").len(), 1);
        assert_eq!(game_titled(&catalog, "Gamma Tactics").len(), 1);
        // And the private record exists either way — it is what a launch reads.
        assert_eq!(catalog.runner_inventory.len(), 3);
    }

    /// An import runs again every time a library is refreshed, so the same way
    /// of starting the same game must not stack up.
    #[test]
    fn importing_twice_does_not_stack_up_ways_to_start_the_same_game() {
        let harness = Harness::new("dedupe-twice", THREE_ROMS);
        harness.configured_profile();
        plant_library_game(&harness, "steam:42", "Alpha Quest");

        let cancelled = AtomicBool::new(false);
        for _ in 0..2 {
            harness
                .service
                .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
                .unwrap();
        }

        let catalog = harness.catalog();
        let host = catalog
            .games
            .iter()
            .find(|game| game.id == "steam:42")
            .unwrap();
        assert_eq!(host.alternate_launch_targets.len(), 1);
        assert_eq!(game_titled(&catalog, "Alpha Quest").len(), 1);
    }

    /// A library that already carries the duplicate heals on the next import,
    /// rather than needing the stray card removed by hand.
    #[test]
    fn a_standalone_card_this_runner_wrote_before_is_folded_in() {
        let harness = Harness::new("dedupe-heal", THREE_ROMS);
        harness.configured_profile();

        let cancelled = AtomicBool::new(false);
        harness
            .service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .unwrap();
        assert_eq!(game_titled(&harness.catalog(), "Alpha Quest").len(), 1);

        // The game arrives in the library by another route afterwards.
        plant_library_game(&harness, "steam:42", "Alpha Quest");
        assert_eq!(game_titled(&harness.catalog(), "Alpha Quest").len(), 2);

        harness
            .service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .unwrap();

        let catalog = harness.catalog();
        let cards = game_titled(&catalog, "Alpha Quest");
        assert_eq!(cards.len(), 1, "the stray card is gone");
        assert_eq!(cards[0].id, "steam:42");
        assert_eq!(cards[0].alternate_launch_targets.len(), 1);
    }

    /// Removing the profile takes away the way of starting it offered, and
    /// leaves the card that was never its own alone.
    #[test]
    fn removing_the_profile_takes_back_the_way_to_start_it_offered() {
        let harness = Harness::new("dedupe-remove", THREE_ROMS);
        harness.configured_profile();
        plant_library_game(&harness, "steam:42", "Alpha Quest");

        let cancelled = AtomicBool::new(false);
        harness
            .service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .unwrap();
        harness.service.delete_profile(ACCEPTED_PROFILE_ID).unwrap();

        let catalog = harness.catalog();
        let host = catalog
            .games
            .iter()
            .find(|game| game.id == "steam:42")
            .expect("the card was never this runner's to remove");
        assert!(host.alternate_launch_targets.is_empty());
        // And the runner's own cards went with it.
        assert!(game_titled(&catalog, "Beta Racer").is_empty());
    }

    // -----------------------------------------------------------------------
    // Profiles
    // -----------------------------------------------------------------------

    /// A profile the plugin rejects is kept with its reason. Throwing it away
    /// would cost the user the application they just picked, and pretending it
    /// was accepted would let it launch.
    #[test]
    fn a_profile_the_plugin_refuses_is_kept_and_cannot_launch() {
        let harness = Harness::new("refused", THREE_ROMS);
        let profile = harness
            .service
            .create_profile(FIXTURE_PLUGIN_ID, "Rejected", &harness.emulator)
            .unwrap();
        assert_eq!(profile.status, RunnerProfileStatus::Rejected);
        assert_eq!(
            profile.status_message.as_deref(),
            Some("This profile was not created by the fixture runner.")
        );
        assert!(harness.catalog().runner_profile(&profile.id).is_some());

        let cancelled = AtomicBool::new(false);
        assert!(matches!(
            harness.service.import_now(&profile.id, &cancelled, |_| {}),
            Err(RunnerHostError::ProfileRefused(_))
        ));
    }

    #[test]
    fn an_application_that_cannot_be_started_never_becomes_a_profile() {
        let harness = Harness::new("app", THREE_ROMS);
        let not_a_program = harness.root.join("notes.txt");
        fs::write(&not_a_program, b"not an emulator").unwrap();

        assert_eq!(
            harness
                .service
                .create_profile(FIXTURE_PLUGIN_ID, "Broken", &not_a_program)
                .unwrap_err(),
            RunnerHostError::ApplicationUnavailable
        );
        // Refused before anything was written, so there is no catalog yet.
        assert!(
            harness
                .service
                .store_for_tests()
                .snapshot()
                .unwrap()
                .runner_profiles
                .is_empty()
        );
        assert!(!harness.catalog_path.exists());
    }

    #[test]
    fn a_disabled_profile_keeps_its_games_and_refuses_to_launch() {
        let harness = Harness::new("disabled", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        harness
            .service
            .set_profile_enabled(ACCEPTED_PROFILE_ID, false)
            .unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::ProfileNotUsable
        );
        assert_eq!(harness.catalog().runner_inventory.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Grants
    // -----------------------------------------------------------------------

    /// Promise 6, as a test: revoking takes a permission away and nothing else.
    #[test]
    fn revoking_a_folder_stops_a_launch_and_keeps_every_game() {
        let harness = Harness::new("revoke", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        let profile = harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, FIXTURE_SLOT)
            .unwrap();
        assert!(!profile.directories[0].granted);
        assert_eq!(profile.game_count, 3);
        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantMissing
        );
        let catalog = harness.catalog();
        assert_eq!(catalog.runner_inventory.len(), 3);
        assert!(catalog.runner_profile(ACCEPTED_PROFILE_ID).is_some());
        // The ledger keeps the row rather than deleting it, so when the folder
        // was readable stays answerable.
        assert!(
            catalog
                .plugin_grants
                .iter()
                .any(|grant| grant.capability == PluginCapability::FilesRead
                    && grant.revoked_at.is_some())
        );

        // And allowing it again is all it takes to get back.
        harness
            .service
            .grant_directory(ACCEPTED_PROFILE_ID, Some(FIXTURE_SLOT), &harness.games)
            .unwrap();
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_ok()
        );
    }

    /// Discovery reads the folder through `host-files`, so without the grant in
    /// force there is nothing for the plugin to page through — and the refusal
    /// comes from the host rather than from an empty answer.
    #[test]
    fn an_import_cannot_run_without_the_folder_grant() {
        let harness = Harness::new("ungranted", THREE_ROMS);
        harness.configured_profile();
        harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, FIXTURE_SLOT)
            .unwrap();

        let cancelled = AtomicBool::new(false);
        assert_eq!(
            harness
                .service
                .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
                .unwrap_err(),
            RunnerHostError::GrantMissing
        );
    }

    #[test]
    fn deleting_a_profile_takes_its_cards_with_it_and_leaves_the_ledger_readable() {
        let harness = Harness::new("delete", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        assert!(harness.service.delete_profile(ACCEPTED_PROFILE_ID).unwrap());

        let catalog = harness.catalog();
        assert!(catalog.runner_profiles.is_empty());
        assert!(catalog.runner_inventory.is_empty());
        assert!(!catalog.games.iter().any(|game| {
            matches!(&game.launch_target, LaunchTarget::Runner { runner_id, .. }
                if runner_id == FIXTURE_PLUGIN_ID)
        }));
        assert!(!catalog.plugin_grants.is_empty());
        assert!(catalog.plugin_grants.iter().all(|grant| !grant.is_active()));
    }

    // -----------------------------------------------------------------------
    // A plugin that lies
    // -----------------------------------------------------------------------

    /// Each of these is the fixture answering `prepare-launch` about something
    /// other than what it was asked. None of them may reach a process.
    #[test]
    fn an_intent_that_does_not_describe_the_call_is_refused() {
        let harness = Harness::new(
            "lies",
            &[
                ("bad-target.rom", "Target"),
                ("bad-mode.rom", "Mode"),
                ("bad-runner.rom", "Runner"),
                ("bad-id.rom", "Identifier"),
            ],
        );
        harness.configured_profile();
        for (reference, file) in [
            ("fixture:bad-target", "bad-target"),
            ("fixture:bad-mode", "bad-mode"),
            ("fixture:bad-runner", "bad-runner"),
            ("fixture:bad-id", "bad-id"),
        ] {
            plant_entry(&harness, reference, file);
            assert!(
                matches!(
                    launch_error(&harness.service, reference),
                    RunnerHostError::Plugin(
                        crate::plugin_runtime::PluginRuntimeError::InvalidResult(_)
                    )
                ),
                "{reference} should have been refused as an unusable result"
            );
        }
    }

    #[test]
    fn a_plugin_that_simply_fails_stops_the_launch_and_writes_nothing() {
        let harness = Harness::new("fails", &[("fail.rom", "Failing Game")]);
        harness.configured_profile();
        plant_entry(&harness, "fixture:fail", "fail");
        let before = harness.catalog();

        assert!(matches!(
            launch_error(&harness.service, "fixture:fail"),
            RunnerHostError::Plugin(_)
        ));
        assert_eq!(harness.catalog(), before);
    }

    /// The fixture asks for a folder it was never given. The refusal is the
    /// host's, it is typed, and it does not become an empty import.
    #[test]
    fn a_plugin_reaching_for_an_ungranted_folder_is_refused_by_the_host() {
        let harness = Harness::new("deny", &[("deny.rom", "Denied")]);
        harness.configured_profile();
        plant_entry(&harness, "fixture:deny", "deny");

        assert!(matches!(
            launch_error(&harness.service, "fixture:deny"),
            RunnerHostError::Plugin(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_game_file_swapped_for_a_link_out_of_its_folder_cannot_be_launched() {
        let harness = Harness::new("swap", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        fs::remove_file(harness.games.join("alpha.rom")).unwrap();
        std::os::unix::fs::symlink(
            harness.root.join("secret.txt"),
            harness.games.join("alpha.rom"),
        )
        .unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GameOutsideScope
        );
    }

    // -----------------------------------------------------------------------
    // Imports
    // -----------------------------------------------------------------------

    /// The plan's exit test: a runner can import a game and start it again after
    /// a restart without rescanning the whole library.
    #[test]
    fn an_import_resumes_from_its_persisted_cursor_after_a_restart() {
        let paged = RunnerImportLimits {
            page_size: 2,
            ..RunnerImportLimits::default()
        };
        let harness = Harness::with_limits(
            "resume",
            &[
                ("alpha.rom", "Alpha Quest"),
                ("beta.rom", "Beta Racer"),
                ("gamma.rom", "Gamma Tactics"),
                ("delta.rom", "Delta Drift"),
                ("epsilon.rom", "Epsilon Echo"),
            ],
            paged,
        );
        harness.configured_profile();

        // Stop after the first page, exactly as a user pressing cancel does.
        let cancelled = AtomicBool::new(false);
        let first = harness
            .service
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {
                cancelled.store(true, Ordering::Release);
            })
            .unwrap();
        assert!(first.cancelled);
        assert_eq!(first.progress.pages, 1);
        assert_eq!(first.progress.imported, 2);
        assert!(first.resumed_from.is_none());

        let persisted = harness.catalog();
        let cursor = persisted
            .runner_profile(ACCEPTED_PROFILE_ID)
            .unwrap()
            .import_cursor
            .clone()
            .expect("the cursor is persisted with the page it belongs to");
        assert!(
            !persisted
                .runner_profile(ACCEPTED_PROFILE_ID)
                .unwrap()
                .import_complete
        );

        // Nothing in memory survives a restart, so the cursor on disk is all
        // the second run has — and it is enough.
        let restarted = harness.restart(paged);
        let cancelled = AtomicBool::new(false);
        let second = restarted
            .import_now(ACCEPTED_PROFILE_ID, &cancelled, |_| {})
            .unwrap();
        assert_eq!(second.resumed_from.as_deref(), Some(cursor.as_str()));
        assert!(second.complete);
        // The three it had not seen, and not one of the two it had.
        assert_eq!(second.progress.imported, 3);
        assert_eq!(second.progress.refreshed, 0);
        assert_eq!(harness.catalog().runner_inventory.len(), 5);
    }

    #[test]
    fn importing_the_same_library_twice_refreshes_rather_than_duplicates() {
        let harness = Harness::new("idempotent", THREE_ROMS);
        harness.configured_profile();
        let first = harness.import();
        assert_eq!(first.progress.imported, 3);
        let after_first = harness.catalog();

        let second = harness.import();
        assert_eq!(second.progress.imported, 0);
        assert_eq!(second.progress.refreshed, 3);
        let after_second = harness.catalog();
        assert_eq!(after_second.runner_inventory.len(), 3);
        assert_eq!(after_second.games.len(), after_first.games.len());
        // The card identity is derived from the external reference, so the
        // second pass lands on the same rows.
        assert_eq!(
            after_second
                .games
                .iter()
                .map(|game| game.id.clone())
                .collect::<Vec<_>>(),
            after_first
                .games
                .iter()
                .map(|game| game.id.clone())
                .collect::<Vec<_>>()
        );
    }

    /// One candidate the host cannot resolve is dropped from its page. A library
    /// with an ambiguous filename in it still imports.
    #[test]
    fn a_candidate_the_host_cannot_resolve_is_skipped_without_losing_the_page() {
        let harness = Harness::new(
            "skip",
            &[
                ("alpha.rom", "Alpha Quest"),
                ("alpha.bin", "Alpha Again"),
                ("beta.rom", "Beta Racer"),
            ],
        );
        harness.configured_profile();

        let outcome = harness.import();
        assert_eq!(outcome.progress.skipped, 1);
        assert_eq!(outcome.progress.imported, 1);
        assert!(outcome.complete);
        assert_eq!(harness.catalog().runner_inventory.len(), 1);
    }

    #[test]
    fn a_cancelled_import_job_reports_that_it_kept_its_place() {
        let paged = RunnerImportLimits {
            page_size: 1,
            ..RunnerImportLimits::default()
        };
        let harness = Harness::with_limits("job", THREE_ROMS, paged);
        harness.configured_profile();

        let job_id = harness.service.start_import(ACCEPTED_PROFILE_ID).unwrap();
        harness.service.cancel_import(&job_id).unwrap();
        let view = loop {
            let view = harness.service.import_status(&job_id).unwrap();
            if view.phase != RunnerImportPhase::Running {
                break view;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(matches!(
            view.phase,
            RunnerImportPhase::Cancelled | RunnerImportPhase::Ready
        ));
        assert_eq!(view.profile_id, ACCEPTED_PROFILE_ID);
    }

    // -----------------------------------------------------------------------
    // View models
    // -----------------------------------------------------------------------

    /// The Plugins surface is the one place these values are rendered, and a
    /// path in any of them would be a path in the WebView.
    #[test]
    fn no_view_model_carries_a_filesystem_path() {
        let harness = Harness::new("views", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        let runners = harness.service.installed_runners().unwrap();
        assert_eq!(runners.len(), 1);
        assert_eq!(runners[0].profiles.len(), 1);
        // Nothing in a view model is a location, so nothing in it needs a path
        // separator. Asserting on the character rather than on this harness's own
        // paths is what makes the test fail if a field is ever added that leaks
        // one.
        let json = serde_json::to_string(&runners).unwrap();
        assert!(!json.contains('/'), "{json}");
        assert!(json.contains("Fixture Emulator"));
        assert!(json.contains(ACCEPTED_PROFILE_ID));
    }
    // -----------------------------------------------------------------------
    // Revocation has to be per profile, and it has to stick
    // -----------------------------------------------------------------------

    /// A component hard-codes the slot it asks `host-files` for, so every
    /// profile of one plugin names its folder under the same id. A permission
    /// keyed by the slot alone therefore has another profile standing in for
    /// the one the user just revoked, and revoking does nothing at all.
    #[test]
    fn revoking_one_profiles_folder_leaves_the_other_profile_untouched() {
        let harness = Harness::new("shared-slot", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        harness.second_profile("other-games", &[("delta.rom", "Delta Drift")]);

        harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, FIXTURE_SLOT)
            .unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantMissing
        );
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, SECOND_PROFILE_ID, "delta")
                .is_ok(),
            "the other profile's folder was never revoked"
        );
    }

    /// A permission taken away stays away until the user gives it again.
    /// Renaming a profile, enabling it, or revalidating it says nothing about
    /// folders, so none of them may put one back.
    #[test]
    fn a_revoked_folder_is_not_restored_by_editing_the_profile() {
        let harness = Harness::new("revoke-rename", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, FIXTURE_SLOT)
            .unwrap();

        harness
            .service
            .rename_profile(ACCEPTED_PROFILE_ID, "Renamed")
            .unwrap();
        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantMissing,
            "a rename re-granted the folder"
        );

        harness
            .service
            .set_profile_enabled(ACCEPTED_PROFILE_ID, false)
            .unwrap();
        harness
            .service
            .set_profile_enabled(ACCEPTED_PROFILE_ID, true)
            .unwrap();
        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantMissing,
            "a disable/enable round trip re-granted the folder"
        );
    }

    /// Allowing a second folder is consent about that folder and nothing else.
    #[test]
    fn a_revoked_folder_is_not_restored_by_allowing_a_different_one() {
        let harness = Harness::new("revoke-second", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, FIXTURE_SLOT)
            .unwrap();

        let elsewhere = harness.root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        harness
            .service
            .grant_directory(ACCEPTED_PROFILE_ID, Some("extra"), &elsewhere)
            .unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantMissing,
            "allowing another folder re-granted the revoked one"
        );
    }

    /// Creating a second profile restates nothing about the first.
    #[test]
    fn a_revoked_folder_is_not_restored_by_adding_a_profile() {
        let harness = Harness::new("revoke-newprofile", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, FIXTURE_SLOT)
            .unwrap();
        harness.second_profile("other-games", &[("delta.rom", "Delta Drift")]);

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantMissing,
            "adding a profile re-granted a revoked folder"
        );
    }

    // -----------------------------------------------------------------------
    // Grants belong to the package that was consented to
    // -----------------------------------------------------------------------

    /// The package the user allowed was release-signed. A package that takes
    /// its id afterwards without that signature is a different thing, and the
    /// permissions do not come with the name.
    #[test]
    fn grants_do_not_survive_a_signed_package_being_replaced_by_an_unsigned_one() {
        let harness = Harness::new("resign", THREE_ROMS);
        harness.configured_profile();
        harness.import();
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_ok()
        );

        fs::remove_file(harness.trust_marker()).unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GrantStale
        );
        // And nothing was destroyed by noticing: the profile and its games are
        // where the user left them.
        assert_eq!(harness.catalog().runner_inventory.len(), 3);
        assert!(
            harness
                .catalog()
                .runner_profile(ACCEPTED_PROFILE_ID)
                .is_some()
        );
    }

    /// A component that is not the one the profile was validated against has
    /// not been judged by anything. The verdict goes back to unvalidated rather
    /// than carrying over to code the plugin never showed the host.
    #[test]
    fn a_profile_validated_against_another_build_is_not_launchable() {
        let harness = Harness::new("rebuild", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        // What an in-place replacement looks like from the catalog's side: the
        // profile remembers a component that is no longer installed.
        harness
            .service
            .store_for_tests()
            .commit(|catalog| {
                let profile = catalog
                    .runner_profile(ACCEPTED_PROFILE_ID)
                    .cloned()
                    .unwrap();
                catalog.upsert_runner_profile(crate::catalog::RunnerProfile {
                    package_fingerprint: Some("0".repeat(64)),
                    ..profile
                })?;
                Ok(())
            })
            .unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::ProfileNeedsRevalidation
        );
        assert_eq!(harness.catalog().runner_inventory.len(), 3);
    }

    // -----------------------------------------------------------------------
    // The folder that was allowed, and no other
    // -----------------------------------------------------------------------

    /// Re-resolving the folder from its path every launch means a link planted
    /// over one of its parents moves the whole grant, and the emulator is handed
    /// the decoy's file under the real game's name.
    #[cfg(unix)]
    #[test]
    fn replacing_a_parent_with_a_link_does_not_move_the_granted_folder() {
        let harness = Harness::new("parent-link", THREE_ROMS);
        let live = harness.root.join("live");
        fs::create_dir_all(live.join("roms")).unwrap();
        fs::write(live.join("roms/alpha.rom"), b"Alpha Quest").unwrap();
        harness
            .service
            .create_profile_with_id(
                ACCEPTED_PROFILE_ID,
                FIXTURE_PLUGIN_ID,
                "Fixture Runner",
                &harness.emulator,
            )
            .unwrap();
        harness
            .service
            .grant_directory(ACCEPTED_PROFILE_ID, Some(FIXTURE_SLOT), &live.join("roms"))
            .unwrap();
        harness.import();
        assert_eq!(harness.catalog().runner_inventory.len(), 1);

        // The parent is swapped for a link to somewhere else entirely, which is
        // a rename and a symlink — neither of which needs to touch the folder
        // the user actually allowed.
        let decoy = harness.root.join("decoy");
        fs::create_dir_all(decoy.join("roms")).unwrap();
        fs::write(decoy.join("roms/alpha.rom"), b"Not your game").unwrap();
        fs::rename(&live, harness.root.join("live-real")).unwrap();
        std::os::unix::fs::symlink(&decoy, &live).unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GameOutsideScope
        );
    }

    /// The same question without a link in it: the folder was renamed away and
    /// an ordinary directory took its name. The path still canonicalises to
    /// itself, so only the folder's own identity can tell them apart.
    #[cfg(unix)]
    #[test]
    fn a_granted_folder_replaced_by_another_real_folder_is_refused() {
        let harness = Harness::new("folder-swap", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        fs::rename(&harness.games, harness.root.join("games-real")).unwrap();
        fs::create_dir_all(&harness.games).unwrap();
        fs::write(harness.games.join("alpha.rom"), b"Not your game").unwrap();

        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::GameOutsideScope
        );
    }

    // -----------------------------------------------------------------------
    // A folder that is simply not there
    // -----------------------------------------------------------------------

    /// An external drive that is unplugged is not a permissions problem, and it
    /// is not every game's problem either. Only the games on it stop, and the
    /// message says which kind of failure it is.
    #[test]
    fn a_folder_that_is_not_there_blocks_only_the_games_inside_it() {
        let harness = Harness::new("offline", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        let removable = harness.root.join("removable");
        fs::create_dir_all(&removable).unwrap();
        fs::write(removable.join("zeta.rom"), b"Zeta Zone").unwrap();
        harness
            .service
            .grant_directory(ACCEPTED_PROFILE_ID, Some("removable"), &removable)
            .unwrap();
        plant_entry_in(&harness, "zeta", "zeta.rom", "removable", &removable);

        fs::remove_dir_all(&removable).unwrap();

        assert_eq!(
            launch_error(&harness.service, "zeta"),
            RunnerHostError::DirectoryUnavailable
        );
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_ok(),
            "one missing folder blocked a game in a folder that is still here"
        );
    }

    // -----------------------------------------------------------------------
    // The junction with the installer
    //
    // Four guarantees, each the thing E2 could narrow from its side but not
    // close. The installer decides when a package has stopped being the package
    // a permission was given to; this service decides what that costs. Both
    // halves run here, wired exactly as `lib.rs` wires them.
    // -----------------------------------------------------------------------

    /// Trust is a statement about bytes, not about a file next to them.
    ///
    /// The check used to be `.staging/trusted/<id>.is_file()`, which would say
    /// "signed" for any marker anyone dropped there — including a stale one
    /// naming a component that is no longer installed. The installer's record
    /// now names the digest it was earned by and is written inside the install
    /// transaction, so the answer moves with the component.
    #[test]
    fn a_package_is_signed_only_for_the_component_the_transaction_accepted() {
        let harness = Harness::new("junction-bytes", THREE_ROMS);
        let (installer, _) = harness.installer();

        // A bare marker of the shape the old check accepted, planted by hand.
        fs::write(harness.trust_marker(), b"1").unwrap();
        assert!(
            !harness
                .service
                .package(FIXTURE_PLUGIN_ID)
                .unwrap()
                .identity()
                .trusted,
            "a file that merely exists is not a signature"
        );

        // A real signed install writes a record that names the component.
        harness
            .install(
                &installer,
                PackageChannel::Official {
                    signer: "orivo-release-v1".into(),
                },
            )
            .expect("installs");
        let identity = harness.service.package(FIXTURE_PLUGIN_ID).unwrap();
        assert!(identity.identity().trusted);
        assert_eq!(identity.identity().fingerprint, fixture_component_sha256());

        // And the record answers about the component, not the id: the digest
        // the host is about to invoke is the question.
        assert!(
            !crate::plugin_installer::component_channel(
                &harness.plugin_root,
                FIXTURE_PLUGIN_ID,
                &"0".repeat(64),
            )
            .is_official()
        );
    }

    /// An uninstall takes the permissions and leaves the library. Same rule as
    /// `forgetting_a_plugin_takes_its_permissions_and_leaves_its_games`, but
    /// reached the way a user reaches it — through the installer — so the hook
    /// is proved to be wired rather than proved to exist.
    #[test]
    fn uninstalling_through_the_installer_revokes_the_grants_and_keeps_the_games() {
        let harness = Harness::new("junction-uninstall", THREE_ROMS);
        let (installer, _) = harness.installer();
        harness
            .install(
                &installer,
                PackageChannel::Official {
                    signer: "orivo-release-v1".into(),
                },
            )
            .expect("installs");
        harness.configured_profile();
        harness.import();
        assert!(harness.grants_active() > 0);
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_ok()
        );

        installer.uninstall(FIXTURE_PLUGIN_ID).expect("uninstalls");

        let catalog = harness.catalog();
        assert_eq!(
            harness.grants_active(),
            0,
            "the permissions went with the package"
        );
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap();
        assert_eq!(profile.status, RunnerProfileStatus::Unvalidated);
        assert_eq!(
            profile.game_directories.len(),
            1,
            "the folder the user picked stays"
        );
        assert_eq!(
            catalog.runner_inventory.len(),
            3,
            "and so does every game imported"
        );
    }

    /// The same package arriving again the same way is not a new package, so
    /// nobody is told and nothing is asked again. This is the case that has to
    /// keep working, or every reinstall would cost the user their folders.
    ///
    /// The announcement count is the point. Without it the test passes even if
    /// `grant_verdict` always answered `Revalidate`, because an install that
    /// changes nothing raises no event to apply a verdict to — which proves the
    /// comparison, not the rule. The rule's `Keep` branch needs a *second*
    /// version of the component to reach end to end, and the reference fixture
    /// reports exactly one; it is pinned in
    /// `permissions_follow_a_package_only_forward_and_only_under_one_signer`.
    #[test]
    fn a_reinstall_of_the_same_package_tells_nobody_and_costs_nothing() {
        let harness = Harness::new("junction-keep", THREE_ROMS);
        let (installer, announced) = harness.installer();
        let official = PackageChannel::Official {
            signer: "orivo-release-v1".into(),
        };
        harness
            .install(&installer, official.clone())
            .expect("installs");
        harness.configured_profile();
        harness.import();
        let before = harness.grants_active();
        assert!(before > 0);
        let announcements = announced.load(Ordering::Relaxed);

        harness
            .install(&installer, official)
            .expect("installs again");

        assert_eq!(
            announced.load(Ordering::Relaxed),
            announcements,
            "the same bytes under the same channel are not a change"
        );
        assert_eq!(harness.grants_active(), before);
        assert_eq!(
            harness
                .catalog()
                .runner_profile(ACCEPTED_PROFILE_ID)
                .unwrap()
                .status,
            RunnerProfileStatus::Valid
        );
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_ok()
        );
    }

    /// A rollback reaches a version the user had, which is exactly why it must
    /// not be assumed to be the version they consented to *for these folders*.
    /// Here it goes back to an unsigned build: the signature that carried the
    /// consent forward is gone, so the profile returns to "needs revalidation"
    /// with its folders and its games intact.
    #[test]
    fn a_rollback_suspends_the_grants_without_touching_the_library() {
        let harness = Harness::new("junction-rollback", THREE_ROMS);
        let (installer, _) = harness.installer();
        // The harness writes a signed package by hand; start from nothing, so
        // the two versions below are the only ones in play.
        installer.uninstall(FIXTURE_PLUGIN_ID).unwrap();
        harness
            .install(&installer, PackageChannel::Development)
            .expect("installs");
        harness
            .install(
                &installer,
                PackageChannel::Official {
                    signer: "orivo-release-v1".into(),
                },
            )
            .expect("installs over it, and keeps the first as the way back");
        harness.configured_profile();
        harness.import();
        assert!(harness.grants_active() > 0);
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_ok()
        );

        installer.roll_back(FIXTURE_PLUGIN_ID).expect("rolls back");

        assert_eq!(harness.grants_active(), 0);
        let catalog = harness.catalog();
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap();
        assert_eq!(profile.status, RunnerProfileStatus::Unvalidated);
        assert_eq!(profile.game_directories.len(), 1);
        assert_eq!(catalog.runner_inventory.len(), 3);
        assert!(
            harness
                .service
                .launch(FIXTURE_PLUGIN_ID, ACCEPTED_PROFILE_ID, "alpha")
                .is_err(),
            "a suspended grant is not a usable one"
        );
    }

    /// The uninstall hook `plugin_installer` has to call. What goes is the
    /// permissions and the verdicts; what stays is everything the user made.
    #[test]
    fn forgetting_a_plugin_takes_its_permissions_and_leaves_its_games() {
        let harness = Harness::new("forget", THREE_ROMS);
        harness.configured_profile();
        harness.import();

        harness.service.forget_plugin(FIXTURE_PLUGIN_ID).unwrap();

        let catalog = harness.catalog();
        assert!(catalog.plugin_grants.iter().all(|grant| !grant.is_active()));
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap();
        assert_eq!(profile.status, RunnerProfileStatus::Unvalidated);
        assert!(profile.package_fingerprint.is_none());
        assert_eq!(profile.game_directories.len(), 1);
        assert_eq!(catalog.runner_inventory.len(), 3);
        assert_eq!(
            catalog
                .games
                .iter()
                .filter(|game| matches!(&game.launch_target,
                    crate::catalog::LaunchTarget::Runner { runner_id, .. }
                        if runner_id == FIXTURE_PLUGIN_ID))
                .count(),
            3
        );
        assert_eq!(
            launch_error(&harness.service, "alpha"),
            RunnerHostError::ProfileNeedsRevalidation
        );
    }

    /// The write half of an import must not do filesystem work: a plugin that
    /// answers with a page of ids naming nothing would otherwise hold the
    /// catalog's write lease for a directory scan per candidate. Committing a
    /// page whose folder has since been deleted is how that is proven — the
    /// commit still succeeds, because it never looks.
    #[test]
    fn committing_a_page_touches_no_filesystem() {
        let harness = Harness::new("no-io-commit", THREE_ROMS);
        harness.configured_profile();
        let catalog = harness.catalog();
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap().clone();
        let page = crate::plugin_runtime::PluginDiscoveryPage {
            games: Vec::new(),
            next_cursor: Some("alpha.rom".into()),
            complete: false,
        };
        let entry = crate::catalog::RunnerGameInventoryEntry {
            profile_id: ACCEPTED_PROFILE_ID.into(),
            game_ref: "alpha".into(),
            title: "Alpha Quest".into(),
            provider_id: FIXTURE_PLUGIN_ID.into(),
            external_id: "alpha".into(),
            game_path: fs::canonicalize(harness.games.join("alpha.rom")).unwrap(),
            directory_grant_id: FIXTURE_SLOT.into(),
            platform: None,
            imported_at: Some(1),
        };

        fs::remove_dir_all(&harness.games).unwrap();

        let committed = crate::runner_host::commit_resolved_page(
            harness.service.store_for_tests(),
            FIXTURE_PLUGIN_ID,
            ACCEPTED_PROFILE_ID,
            &profile.game_directories,
            vec![entry],
            &page,
            1,
            0,
        )
        .unwrap();
        assert_eq!(committed.imported, 1);
        assert_eq!(
            harness
                .catalog()
                .runner_profile(ACCEPTED_PROFILE_ID)
                .unwrap()
                .import_cursor
                .as_deref(),
            Some("alpha.rom")
        );
    }

    /// A page resolved against folders that changed underneath it is not
    /// something to write down: those paths were checked against a grant that
    /// no longer describes the profile.
    #[test]
    fn a_page_resolved_against_other_folders_is_refused() {
        let harness = Harness::new("stale-page", THREE_ROMS);
        harness.configured_profile();
        let stale = vec![crate::catalog::RunnerGrantedDirectory {
            id: FIXTURE_SLOT.into(),
            path: harness.root.join("somewhere-else"),
            device: None,
            inode: None,
        }];
        let page = crate::plugin_runtime::PluginDiscoveryPage {
            games: Vec::new(),
            next_cursor: None,
            complete: true,
        };

        assert!(
            crate::runner_host::commit_resolved_page(
                harness.service.store_for_tests(),
                FIXTURE_PLUGIN_ID,
                ACCEPTED_PROFILE_ID,
                &stale,
                Vec::new(),
                &page,
                1,
                0,
            )
            .is_err()
        );
    }

    /// A slot is joined to a profile id to key the ledger, so one carrying the
    /// separator could spell a different profile's permission.
    #[test]
    fn a_folder_slot_cannot_spell_another_profiles_key() {
        let harness = Harness::new("slot-grammar", THREE_ROMS);
        harness.configured_profile();
        let elsewhere = harness.root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();

        assert_eq!(
            harness
                .service
                .grant_directory(
                    ACCEPTED_PROFILE_ID,
                    Some("fixture-profile-2:fixture-games"),
                    &elsewhere,
                )
                .unwrap_err(),
            RunnerHostError::GrantRefused
        );
    }
    // -----------------------------------------------------------------------
    // An import may only look where it is still allowed to look
    // -----------------------------------------------------------------------

    /// A launch refuses an entry whose folder was revoked, but the import was
    /// still walking every folder the profile records. Contents the plugin has
    /// no permission to reach therefore reached the resolution — here by making
    /// a name ambiguous and losing the game that *was* allowed.
    #[test]
    fn a_revoked_folder_does_not_take_part_in_an_import() {
        let harness = Harness::new("revoked-import", THREE_ROMS);
        harness.configured_profile();
        let extra = harness.root.join("extra");
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("alpha.bin"), b"Not your game").unwrap();
        fs::write(extra.join("zeta.rom"), b"Zeta Zone").unwrap();
        harness
            .service
            .grant_directory(ACCEPTED_PROFILE_ID, Some("extra"), &extra)
            .unwrap();
        harness
            .service
            .revoke_directory(ACCEPTED_PROFILE_ID, "extra")
            .unwrap();

        let outcome = harness.import();
        assert_eq!(
            outcome.progress.skipped, 0,
            "a revoked folder made a name ambiguous"
        );
        assert_eq!(outcome.progress.imported, 3);
        // And nothing from the revoked folder became a library card.
        let catalog = harness.catalog();
        assert!(
            catalog
                .runner_inventory
                .iter()
                .all(|entry| entry.directory_grant_id == FIXTURE_SLOT),
            "an entry was resolved inside a revoked folder"
        );
    }

    /// The same for a folder that is no longer the folder the user allowed: the
    /// import must not read out of it either.
    #[cfg(unix)]
    #[test]
    fn a_swapped_folder_does_not_take_part_in_an_import() {
        let harness = Harness::new("swapped-import", THREE_ROMS);
        harness.configured_profile();
        let extra = harness.root.join("extra");
        fs::create_dir_all(&extra).unwrap();
        harness
            .service
            .grant_directory(ACCEPTED_PROFILE_ID, Some("extra"), &extra)
            .unwrap();
        // Renamed away, and an ordinary folder built where it stood.
        fs::rename(&extra, harness.root.join("extra-real")).unwrap();
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("alpha.bin"), b"Not your game").unwrap();

        let outcome = harness.import();
        assert_eq!(outcome.progress.skipped, 0);
        assert_eq!(outcome.progress.imported, 3);
        assert!(
            harness
                .catalog()
                .runner_inventory
                .iter()
                .all(|entry| entry.directory_grant_id == FIXTURE_SLOT)
        );
    }

    /// The plugin's own reads are pinned by the grant resolution, on the
    /// descriptor it opens, and not by a check of ours beside it.
    ///
    /// The difference is a window, not a rule: a folder swapped between a
    /// path-based check and the open that follows it is the folder the plugin
    /// reads. Asking `resolve_pinned` means the answer is about the descriptor
    /// actually handed over, so there is nothing in between to race.
    #[cfg(unix)]
    #[test]
    fn a_swapped_folder_is_refused_by_the_grant_resolution_itself() {
        let harness = Harness::new("pinned", THREE_ROMS);
        harness.configured_profile();
        let package = harness.service.package(FIXTURE_PLUGIN_ID).unwrap();
        let catalog = harness.catalog();
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap();
        let resolved =
            crate::runner_host::resolve_profile_grants(&catalog, &package, profile).unwrap();
        assert!(resolved.granted.contains(FIXTURE_SLOT));
        assert!(resolved.grants.holds(PluginCapability::FilesRead));

        // Renamed away, and an ordinary folder built where it stood: the path
        // still canonicalises to itself, so only the folder's own identity —
        // read from the descriptor the resolution opened — can tell them apart.
        fs::rename(&harness.games, harness.root.join("games-real")).unwrap();
        fs::create_dir_all(&harness.games).unwrap();

        let catalog = harness.catalog();
        let profile = catalog.runner_profile(ACCEPTED_PROFILE_ID).unwrap();
        let resolved =
            crate::runner_host::resolve_profile_grants(&catalog, &package, profile).unwrap();
        assert!(
            !resolved.granted.contains(FIXTURE_SLOT),
            "the grant resolution accepted a folder that is not the one allowed"
        );
        assert!(!resolved.grants.holds(PluginCapability::FilesRead));
    }
}
