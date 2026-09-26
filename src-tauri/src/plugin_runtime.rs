//! Orivo's Wasmtime host for third-party plugin components.
//!
//! Invocation, host functions, grants and `Store` limits live in this one
//! module on purpose. Any one of them shipping without the others turns an
//! opaque launch target into a process, or lets a component run without the
//! ceiling that makes running it safe, so they are written and tested together.
//!
//! Three rules shape everything below.
//!
//! * **The manifest decides what is linked; the grant decides what works.** An
//!   import the manifest never declared is absent from the linker, so a
//!   component asking for it cannot *instantiate* — it never executes an
//!   instruction under a permission its package did not show the user. A
//!   declared but ungranted capability *is* linked, and refuses every call with
//!   a typed WIT error. That split is deliberate: it is what lets Orivo
//!   identify and health-check a plugin, under limits and with zero authority,
//!   before asking the user to grant it anything. Scope — which directory,
//!   which entry — is a per-argument question and is re-checked on every call.
//! * **Every limit belongs to the host.** Fuel, the epoch deadline, memory per
//!   instance, memory across all instances, tables and instance counts are set
//!   here from [`PluginLimits`]. A manifest cannot raise them and a component
//!   cannot observe them.
//! * **A result is untrusted until the host has validated it.** The guest
//!   returns WIT records; the host turns them into the closed types at the
//!   bottom of this file, rejecting anything whose identifiers, sizes or launch
//!   mode it does not already recognise. `PluginLaunchIntent` is closed, and
//!   the executable is resolved from the profile — never from the plugin.
//!
//! Several items below carry `#[allow(dead_code)]`. Each is a seam a following
//! lot consumes — the launch path for the runner requests and
//! `PluginLaunchIntent`, the "Add an emulator" flow for directory grants and
//! `resume`, an adversarial suite for the manual epoch and the journal — and each
//! is exercised by the tests at the bottom of this file. Landing them with the
//! host is the point: a limit or a refusal that arrives after the code it is
//! meant to gate is one that never gated anything.

use crate::plugin_scheduler::{
    JobError, JobHandle, PluginScheduler, SchedulerLimits, SubmitError,
};
use crate::plugin_manifest::{
    CapabilityGrant, CapabilityScope, GrantValidationError, PluginCapability, PluginExtension,
    ValidatedPluginManifest, valid_opaque_id,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use wasmtime::{
    Config, Engine, OptLevel, ResourceLimiter, Store, Trap, UpdateDeadline,
    component::{Component, HasSelf, Linker, types::ComponentItem},
};

pub mod bindings {
    //! Typed bindings generated from `wit/orivo-plugin.wit`.
    //!
    //! Only `runner-plugin` is generated: it is the one world this host slice
    //! can serve, and generating a world Orivo cannot invoke would advertise a
    //! contract that does not exist.
    wasmtime::component::bindgen!({
        world: "runner-plugin",
        path: "../wit",
    });
}

use bindings::{
    RunnerPlugin, RunnerPluginPre,
    exports::orivo::plugin::{plugin_core as wit_core, runner as wit_runner},
    orivo::plugin::{host_files, host_journal, types as wit_types},
};

pub const PLUGIN_WASM_STACK_BYTES: usize = 2 * 1024 * 1024;

/// How often the host advances Wasmtime's epoch. Every deadline below is a
/// whole number of these ticks, so 10 ms is also the resolution at which a
/// runaway component — or a cancellation — is noticed.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);

/// The performance contract's interactive budget. This is a *display* budget:
/// past it the caller stops waiting and shows progress. It is deliberately not
/// the deadline that kills the component — a cold first call legitimately costs
/// more than this, and killing it would make a correct plugin look broken.
#[allow(dead_code)]
pub const INTERACTIVE_DISPLAY_BUDGET: Duration = Duration::from_millis(150);

/// Host calls per invocation. A page of candidates needs a listing and a read
/// per entry; 256 is past any bounded page and far short of a loop.
const MAX_HOST_CALLS_PER_INVOCATION: u32 = 256;
const MAX_DIRECTORY_ENTRIES: usize = 256;
const MAX_HOST_FILE_BYTES: u64 = 1024 * 1024;
/// Total bytes one invocation may read through `host-files`. Without it a
/// plugin could stream a granted directory into its own linear memory until it
/// hit the memory ceiling instead of the read ceiling.
const MAX_HOST_READ_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRY_NAME_BYTES: usize = 255;
const MAX_JOURNAL_MESSAGE_BYTES: usize = 512;
const MAX_JOURNAL_ENTRIES: usize = 256;

/// Result bounds. Identifiers use the catalogue's opaque grammar; free text is
/// bounded and stripped of control characters before it can reach a view model.
const MAX_RESULT_ID_BYTES: usize = 256;
const MAX_RESULT_TEXT_BYTES: usize = 512;
const MAX_RESULT_CURSOR_BYTES: usize = 256;
const MAX_RESULT_PAGE_GAMES: usize = 100;

/// The WIT names this host answers to. A component may import nothing else:
/// there is no WASI here, general or otherwise, so a package that expects a
/// clock, a random source, a socket or a preopened directory is refused before
/// it is ever instantiated.
const HOST_JOURNAL_IMPORT: &str = "orivo:plugin/host-journal@1.0.0";
/// The shared type definitions. Guest bindings import the interface that owns a
/// record even when only its shape is needed, so this name is expected — but it
/// is accepted only while it stays free of functions, because a future revision
/// that added one would be asking the host for behaviour under a name that
/// looks inert.
const TYPES_IMPORT: &str = "orivo:plugin/types@1.0.0";
const HOST_FILES_IMPORT: &str = "orivo:plugin/host-files@1.0.0";
const PLUGIN_CORE_EXPORT: &str = "orivo:plugin/plugin-core@1.0.0";
const RUNNER_EXPORT: &str = "orivo:plugin/runner@1.0.0";

/// How much longer than a probe's own deadline the registry waits for it. The
/// extra room is queueing, not execution: the component itself is already
/// stopped by the epoch deadline.
const PROBE_WAIT_MULTIPLIER: u32 = 4;

/// Consecutive host-visible failures before a plugin is parked. Three is enough
/// to distinguish a flaky call from a broken component without retrying.
pub const DEFAULT_MAX_CONSECUTIVE_FAILURES: u32 = 3;

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// Every ceiling the host imposes on a component, in one injectable value so a
/// test can shrink a deadline to two ticks without reaching into the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginLimits {
    /// Fuel for a short call: identity, health, profile validation, launch
    /// preparation. Measured against the reference fixture, whose heaviest
    /// interactive call burns four orders of magnitude less than this.
    pub interactive_fuel: u64,
    /// Hard deadline for the same calls. Seven times the display budget: long
    /// enough that a cold call is never killed, short enough that a hung
    /// component is a blink rather than a hang.
    pub interactive_deadline: Duration,
    /// Discovery walks a bounded page, so it gets more of both — but it is
    /// still a job with an end, not a background process.
    pub discovery_fuel: u64,
    pub discovery_deadline: Duration,
    /// Identity and health only. The contract calls these short and never
    /// network-backed, and Orivo runs one pair per installed package every time
    /// Settings → Plugins opens, so their budget is the tightest of the three:
    /// the whole pass has to stay well inside a panel opening.
    pub probe_fuel: u64,
    pub probe_deadline: Duration,
    /// Linear memory one instance may reach. A page of candidates is kilobytes;
    /// this leaves room for a guest allocator's slack and nothing like a cache.
    pub instance_memory_bytes: usize,
    /// Linear memory every live instance may reach *together*. Bounds the whole
    /// plugin surface, not one plugin at a time.
    pub total_memory_bytes: usize,
    pub table_elements: usize,
    pub instances_per_store: usize,
    pub tables_per_store: usize,
    pub memories_per_store: usize,
    pub epoch_tick: Duration,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            interactive_fuel: 50_000_000,
            interactive_deadline: Duration::from_millis(1_000),
            discovery_fuel: 500_000_000,
            discovery_deadline: Duration::from_millis(5_000),
            probe_fuel: 5_000_000,
            probe_deadline: Duration::from_millis(250),
            instance_memory_bytes: 64 * 1024 * 1024,
            total_memory_bytes: 256 * 1024 * 1024,
            table_elements: 10_000,
            // A component is several core modules plus the canonical ABI
            // adapters Wasmtime synthesises, so this is not one instance.
            instances_per_store: 32,
            tables_per_store: 8,
            memories_per_store: 4,
            epoch_tick: EPOCH_TICK,
        }
    }
}

impl PluginLimits {
    fn ticks(&self, deadline: Duration) -> u64 {
        let tick = self.epoch_tick.as_nanos().max(1);
        (deadline.as_nanos().div_ceil(tick) as u64).max(1)
    }
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// Which correlation a journal line belongs to. Every grant decision, limit and
/// outcome carries one so a user-visible failure can be traced back to the call
/// that caused it without logging the arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CorrelationId(pub u64);

impl std::fmt::Display for CorrelationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

pub fn next_correlation_id() -> CorrelationId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    CorrelationId(NEXT.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub correlation_id: CorrelationId,
    pub plugin_id: String,
    pub decision: &'static str,
    pub detail: String,
}

/// A bounded ring of host decisions. It is deliberately in memory and capped:
/// the journal exists to explain the last failure to a user and to let a test
/// assert that a refusal was recorded, not to become a log file a plugin can
/// grow. Settings → Plugins reads it in a later slice.
#[derive(Debug, Default)]
pub struct PluginJournal {
    entries: Mutex<VecDeque<JournalEntry>>,
}

impl PluginJournal {
    pub fn record(
        &self,
        correlation_id: CorrelationId,
        plugin_id: &str,
        decision: &'static str,
        detail: impl Into<String>,
    ) {
        let entry = JournalEntry {
            correlation_id,
            plugin_id: plugin_id.to_owned(),
            decision,
            detail: detail.into(),
        };
        eprintln!(
            "orivo plugin [{}] {} {}: {}",
            entry.correlation_id, entry.plugin_id, entry.decision, entry.detail
        );
        if let Ok(mut entries) = self.entries.lock() {
            if entries.len() == MAX_JOURNAL_ENTRIES {
                entries.pop_front();
            }
            entries.push_back(entry);
        }
    }

    #[allow(dead_code)]
    pub fn entries(&self) -> Vec<JournalEntry> {
        self.entries
            .lock()
            .map(|entries| entries.iter().cloned().collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

/// What one invocation may reach for, in two layers.
///
/// `declared` is what the package asked the user for and therefore what the
/// linker will provide at all. `granted` is what the user actually said yes to,
/// and it is what a host function checks before doing anything. A capability can
/// be declared and not granted — that is the state every plugin is in between
/// being installed and being configured — but never granted without being
/// declared, because `validate_grant` refuses that pairing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginGrants {
    declared: BTreeSet<PluginCapability>,
    granted: BTreeSet<PluginCapability>,
    directories: BTreeMap<String, PathBuf>,
}

impl PluginGrants {
    /// Nothing at all: not even an import. Used where the caller has no
    /// manifest to speak for the component.
    #[allow(dead_code)]
    pub fn none() -> Self {
        Self::default()
    }

    /// Everything the manifest declared, granted for nothing. This is what the
    /// registry probes with: the component can start and describe itself, and
    /// every capability call it makes is refused.
    pub fn declared_only(manifest: &ValidatedPluginManifest) -> Self {
        Self {
            declared: manifest.manifest().capabilities.iter().copied().collect(),
            granted: BTreeSet::new(),
            directories: BTreeMap::new(),
        }
    }

    /// `directories` maps an opaque directory-grant id to the path the host
    /// resolved for it. A grant naming an id that is not in the map is refused:
    /// a persisted grant that outlived its folder must not silently widen to
    /// another one.
    #[allow(dead_code)]
    pub fn resolve(
        manifest: &ValidatedPluginManifest,
        grants: &[CapabilityGrant],
        directories: &BTreeMap<String, PathBuf>,
    ) -> Result<Self, GrantValidationError> {
        let mut resolved = Self::declared_only(manifest);
        for grant in grants {
            manifest.validate_grant(grant)?;
            resolved.granted.insert(grant.capability);
            if let CapabilityScope::DirectoryGrants(ids) = &grant.scope {
                for id in ids {
                    let path = directories
                        .get(id)
                        .ok_or(GrantValidationError::InvalidScope(grant.capability))?;
                    resolved.directories.insert(id.clone(), path.clone());
                }
            }
        }
        Ok(resolved)
    }

    pub fn declares(&self, capability: PluginCapability) -> bool {
        self.declared.contains(&capability)
    }

    pub fn holds(&self, capability: PluginCapability) -> bool {
        self.granted.contains(&capability)
    }

    fn directory(&self, id: &str) -> Option<&Path> {
        self.directories.get(id).map(PathBuf::as_path)
    }
}

// ---------------------------------------------------------------------------
// Memory ceiling
// ---------------------------------------------------------------------------

/// Bytes committed by every live plugin instance. Shared by the runtime so the
/// per-instance ceiling cannot be multiplied by opening more stores.
#[derive(Debug, Default)]
struct MemoryBudget {
    committed: AtomicUsize,
}

impl MemoryBudget {
    /// A single compare-and-swap loop rather than `fetch_add` and a rollback:
    /// an over-commit must never be briefly visible to a concurrent instance,
    /// or two stores can each be told they fit inside the last free page.
    fn charge(&self, extra: usize, ceiling: usize) -> bool {
        let mut current = self.committed.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(extra) else {
                return false;
            };
            if next > ceiling {
                return false;
            }
            match self.committed.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self, bytes: usize) {
        self.committed.fetch_sub(bytes, Ordering::AcqRel);
    }
}

/// The `ResourceLimiter` for one store. It refuses growth rather than trapping
/// so a well-written guest can fail its own allocation gracefully; `hit_limit`
/// is what lets the host report the resulting abort as a memory limit instead
/// of an anonymous trap.
#[derive(Debug)]
struct StoreMemoryGuard {
    limits: PluginLimits,
    budget: Arc<MemoryBudget>,
    charged: usize,
    hit_limit: bool,
}

impl ResourceLimiter for StoreMemoryGuard {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.limits.instance_memory_bytes {
            self.hit_limit = true;
            return Ok(false);
        }
        let extra = desired.saturating_sub(current);
        if !self
            .budget
            .charge(extra, self.limits.total_memory_bytes)
        {
            self.hit_limit = true;
            return Ok(false);
        }
        self.charged = self.charged.saturating_add(extra);
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.limits.table_elements {
            self.hit_limit = true;
            return Ok(false);
        }
        Ok(true)
    }

    fn instances(&self) -> usize {
        self.limits.instances_per_store
    }

    fn tables(&self) -> usize {
        self.limits.tables_per_store
    }

    fn memories(&self) -> usize {
        self.limits.memories_per_store
    }
}

impl Drop for StoreMemoryGuard {
    fn drop(&mut self) {
        self.budget.release(self.charged);
    }
}

// ---------------------------------------------------------------------------
// Epoch
// ---------------------------------------------------------------------------

/// Why a component stopped without returning. Wasmtime reports both a blown
/// deadline and a cancellation as `Trap::Interrupt`, so the host records which
/// one it asked for: a user pressing Escape is not a misbehaving plugin, and
/// only one of the two counts towards `degraded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interruption {
    Deadline,
    Cancelled,
}

#[derive(Debug, Default)]
struct TickerState {
    in_flight: usize,
    stop: bool,
}

/// Advances Wasmtime's epoch while — and only while — a component is running.
/// An idle Orivo has no ticking thread, so enabling plugins does not cost a
/// wakeup every 10 ms for the rest of the session.
struct EpochTicker {
    engine: Engine,
    state: Mutex<TickerState>,
    wake: Condvar,
    tick: Duration,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for EpochTicker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EpochTicker")
            .field("tick", &self.tick)
            .finish_non_exhaustive()
    }
}

impl EpochTicker {
    fn new(engine: Engine, tick: Duration) -> Arc<Self> {
        Arc::new(Self {
            engine,
            state: Mutex::new(TickerState::default()),
            wake: Condvar::new(),
            tick,
            thread: Mutex::new(None),
        })
    }

    fn spawn(self: &Arc<Self>) {
        let ticker = Arc::clone(self);
        let handle = thread::Builder::new()
            .name("orivo-plugin-epoch".into())
            .spawn(move || ticker.run())
            .ok();
        if let Ok(mut slot) = self.thread.lock() {
            *slot = handle;
        }
    }

    fn run(&self) {
        loop {
            {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                while !state.stop && state.in_flight == 0 {
                    let Ok(next) = self.wake.wait(state) else {
                        return;
                    };
                    state = next;
                }
                if state.stop {
                    return;
                }
            }
            thread::sleep(self.tick);
            self.engine.increment_epoch();
        }
    }

    fn enter(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.in_flight += 1;
        }
        self.wake.notify_all();
    }

    fn leave(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
        }
    }

    fn stop(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stop = true;
        }
        self.wake.notify_all();
        let handle = self.thread.lock().ok().and_then(|mut slot| slot.take());
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// Keeps the ticker awake for the length of one invocation.
struct TickerLease<'ticker>(&'ticker EpochTicker);

impl EpochTicker {
    fn lease(&self) -> TickerLease<'_> {
        self.enter();
        TickerLease(self)
    }
}

impl Drop for TickerLease<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

/// Who advances the epoch. `Manual` exists so a test can decide exactly when a
/// deadline expires instead of racing a sleeping thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum EpochMode {
    Threaded,
    Manual,
}

// ---------------------------------------------------------------------------
// Store data
// ---------------------------------------------------------------------------

/// Everything a host function is allowed to know. There is no path, no
/// `AppHandle`, no catalogue and no keychain here: a host import can only do
/// what these fields make possible.
struct HostState {
    plugin_id: String,
    correlation_id: CorrelationId,
    grants: PluginGrants,
    journal: Arc<PluginJournal>,
    memory: StoreMemoryGuard,
    cancel: Arc<AtomicBool>,
    ticks_remaining: u64,
    interruption: Option<Interruption>,
    host_calls: u32,
    bytes_read: u64,
}

impl HostState {
    fn spend_host_call(&mut self) -> Result<(), wit_types::PluginError> {
        if self.host_calls >= MAX_HOST_CALLS_PER_INVOCATION {
            self.journal.record(
                self.correlation_id,
                &self.plugin_id,
                "host-call-budget",
                "refused: the invocation exhausted its host-call budget",
            );
            return Err(plugin_error(
                wit_types::PluginErrorCode::RateLimited,
                "This plugin made too many host requests in one call.",
            ));
        }
        self.host_calls += 1;
        Ok(())
    }

    /// Resolving a grant is the only place a plugin's opaque id becomes a path,
    /// and it fails closed twice: once if the capability was never granted, and
    /// once if this particular id is outside the granted scope.
    fn granted_directory(&self, grant: &str) -> Result<PathBuf, wit_types::PluginError> {
        if !self.grants.holds(PluginCapability::FilesRead) {
            self.journal.record(
                self.correlation_id,
                &self.plugin_id,
                "capability-refused",
                "files_read is not granted",
            );
            return Err(plugin_error(
                wit_types::PluginErrorCode::PermissionDenied,
                "This plugin is not allowed to read files.",
            ));
        }
        match self.grants.directory(grant) {
            Some(path) => Ok(path.to_path_buf()),
            None => {
                self.journal.record(
                    self.correlation_id,
                    &self.plugin_id,
                    "scope-refused",
                    "a directory grant outside the approved scope was requested",
                );
                Err(plugin_error(
                    wit_types::PluginErrorCode::PermissionDenied,
                    "This plugin asked for a folder you have not allowed.",
                ))
            }
        }
    }
}

fn plugin_error(
    code: wit_types::PluginErrorCode,
    message: &str,
) -> wit_types::PluginError {
    wit_types::PluginError {
        code,
        message: message.to_owned(),
        retryable: false,
    }
}

impl host_journal::Host for HostState {
    fn log(&mut self, level: host_journal::JournalLevel, message: String) {
        let level = match level {
            host_journal::JournalLevel::Debug => "debug",
            host_journal::JournalLevel::Info => "info",
            host_journal::JournalLevel::Warning => "warning",
            host_journal::JournalLevel::Error => "error",
        };
        // The message is guest-controlled text. Truncating on a character
        // boundary keeps a malicious plugin from filling the ring with one line
        // and keeps the journal printable.
        let mut detail = String::with_capacity(MAX_JOURNAL_MESSAGE_BYTES);
        detail.push_str(level);
        detail.push_str(": ");
        for character in message.chars().filter(|c| !c.is_control()) {
            if detail.len() + character.len_utf8() > MAX_JOURNAL_MESSAGE_BYTES {
                break;
            }
            detail.push(character);
        }
        self.journal
            .record(self.correlation_id, &self.plugin_id, "plugin-log", detail);
    }
}

impl host_files::Host for HostState {
    fn list_directory(
        &mut self,
        grant: String,
    ) -> Result<Vec<host_files::DirectoryEntry>, wit_types::PluginError> {
        self.spend_host_call()?;
        let root = self.granted_directory(&grant)?;
        let Ok(entries) = fs::read_dir(&root) else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::Unavailable,
                "That folder is no longer readable.",
            ));
        };
        let mut listing = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            if listing.len() == MAX_DIRECTORY_ENTRIES {
                break;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            // `metadata` follows links; `symlink_metadata` is what says whether
            // this entry *is* one. A link is skipped rather than resolved so a
            // granted folder cannot be used as a door to an ungranted one.
            let Ok(raw) = entry.path().symlink_metadata() else {
                continue;
            };
            if raw.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() > MAX_ENTRY_NAME_BYTES || !valid_entry_name(&name) {
                continue;
            }
            listing.push(host_files::DirectoryEntry {
                name,
                byte_size: if metadata.is_file() { metadata.len() } else { 0 },
                directory: metadata.is_dir(),
            });
        }
        listing.sort_by(|left, right| left.name.cmp(&right.name));
        self.journal.record(
            self.correlation_id,
            &self.plugin_id,
            "files-list",
            format!("granted directory listed, {} entries", listing.len()),
        );
        Ok(listing)
    }

    fn read_file(
        &mut self,
        grant: String,
        name: String,
    ) -> Result<Vec<u8>, wit_types::PluginError> {
        self.spend_host_call()?;
        let root = self.granted_directory(&grant)?;
        if name.len() > MAX_ENTRY_NAME_BYTES || !valid_entry_name(&name) {
            return Err(plugin_error(
                wit_types::PluginErrorCode::InvalidInput,
                "That is not a name inside the allowed folder.",
            ));
        }
        let path = root.join(&name);
        let Ok(metadata) = path.symlink_metadata() else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::Unavailable,
                "That file is no longer available.",
            ));
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_HOST_FILE_BYTES {
            return Err(plugin_error(
                wit_types::PluginErrorCode::PermissionDenied,
                "That entry is not a readable file of an allowed size.",
            ));
        }
        if self.bytes_read.saturating_add(metadata.len()) > MAX_HOST_READ_BYTES {
            return Err(plugin_error(
                wit_types::PluginErrorCode::RateLimited,
                "This plugin read too much in one call.",
            ));
        }
        let Ok(bytes) = fs::read(&path) else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::Unavailable,
                "That file could not be read.",
            ));
        };
        self.bytes_read = self.bytes_read.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }
}

/// One path component, and nothing that could leave the granted directory.
fn valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.chars().any(char::is_control)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A typed refusal. Every arm is a decision the host made, not a message a
/// plugin chose: `Plugin` is the one that carries guest text, and it is already
/// bounded and stripped by [`sanitise_text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRuntimeError {
    EngineUnavailable,
    InvalidComponent,
    /// The component compiled but does not export the world this host serves.
    MissingWorld,
    /// The component imports something Orivo has no host function for — a WASI
    /// interface, or a future ABI revision. There is nothing to link it to, so
    /// it cannot be instantiated under any grant.
    UnknownImport,
    /// The component imports a capability its own manifest never declared. The
    /// import was not linked and instantiation was refused: the package asked
    /// the user for less than it needs.
    CapabilityUndeclared(PluginCapability),
    Instantiation,
    /// The plugin is parked after repeated failures and will not be called
    /// again until someone resumes it.
    Paused,
    /// The plugin already has as much work queued as the host will hold.
    Busy,
    /// The component's `get-identity` disagrees with the manifest it shipped in.
    IdentityMismatch,
    FuelExhausted,
    DeadlineExceeded,
    Cancelled,
    MemoryLimit,
    Trapped,
    /// The component returned a WIT error. The message is presentation-safe.
    Plugin {
        code: PluginErrorCode,
        message: String,
        retryable: bool,
    },
    /// The component returned something the host will not write down.
    InvalidResult(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginErrorCode {
    InvalidInput,
    PermissionDenied,
    Unavailable,
    RateLimited,
    Cancelled,
    Internal,
}

impl PluginRuntimeError {
    /// Whether this failure says the plugin is broken. A cancellation is the
    /// user's decision and a refused capability is the host's, so neither may
    /// push a plugin towards `degraded`.
    pub fn counts_as_plugin_failure(&self) -> bool {
        !matches!(
            self,
            Self::Cancelled
                | Self::CapabilityUndeclared(_)
                | Self::EngineUnavailable
                | Self::Paused
                | Self::Busy
        )
    }
}

impl std::fmt::Display for PluginRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EngineUnavailable => {
                write!(formatter, "The Orivo plugin runtime is unavailable.")
            }
            Self::InvalidComponent => write!(
                formatter,
                "The plugin component did not pass WebAssembly validation."
            ),
            Self::MissingWorld => write!(
                formatter,
                "The plugin component does not implement the Orivo runner contract."
            ),
            Self::UnknownImport => write!(
                formatter,
                "The plugin component asks for a host capability Orivo does not provide."
            ),
            Self::CapabilityUndeclared(capability) => write!(
                formatter,
                "This plugin needs a permission its package never declared ({capability:?})."
            ),
            Self::Paused => write!(
                formatter,
                "Orivo paused this plugin after repeated failures. Resume it to try again."
            ),
            Self::Busy => write!(
                formatter,
                "This plugin already has as much work queued as Orivo will hold."
            ),
            Self::IdentityMismatch => write!(
                formatter,
                "This plugin does not match the package it was installed from."
            ),
            Self::Instantiation => {
                write!(formatter, "The plugin component could not be started.")
            }
            Self::FuelExhausted => write!(
                formatter,
                "The plugin used more computation than one call is allowed."
            ),
            Self::DeadlineExceeded => {
                write!(formatter, "The plugin took too long and was stopped.")
            }
            Self::Cancelled => write!(formatter, "The plugin call was cancelled."),
            Self::MemoryLimit => {
                write!(formatter, "The plugin asked for more memory than it may use.")
            }
            Self::Trapped => write!(formatter, "The plugin stopped unexpectedly."),
            Self::Plugin { message, .. } => write!(formatter, "{message}"),
            Self::InvalidResult(reason) => {
                write!(formatter, "The plugin returned an unusable result ({reason}).")
            }
        }
    }
}

impl std::error::Error for PluginRuntimeError {}

impl From<wit_types::PluginError> for PluginRuntimeError {
    fn from(error: wit_types::PluginError) -> Self {
        Self::Plugin {
            code: match error.code {
                wit_types::PluginErrorCode::InvalidInput => PluginErrorCode::InvalidInput,
                wit_types::PluginErrorCode::PermissionDenied => PluginErrorCode::PermissionDenied,
                wit_types::PluginErrorCode::Unavailable => PluginErrorCode::Unavailable,
                wit_types::PluginErrorCode::RateLimited => PluginErrorCode::RateLimited,
                wit_types::PluginErrorCode::Cancelled => PluginErrorCode::Cancelled,
                wit_types::PluginErrorCode::Internal => PluginErrorCode::Internal,
            },
            message: sanitise_text(&error.message, MAX_RESULT_TEXT_BYTES)
                .unwrap_or_else(|| "The plugin reported an error.".into()),
            retryable: error.retryable,
        }
    }
}

// ---------------------------------------------------------------------------
// Host-owned results
// ---------------------------------------------------------------------------

/// How far a verification goes. The contract half reads a component's type and
/// runs nothing, so it is always affordable; the health half runs guest code,
/// which a bounded discovery pass can only pay for a few times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerCheck {
    ContractOnly,
    ContractAndHealth,
}

/// The capabilities a component's imports oblige the host to provide. A
/// manifest that declares fewer than this is a consent screen that under-states
/// what the package does, so the registry refuses the pair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentContract {
    pub required_capabilities: BTreeSet<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginIdentity {
    pub id: String,
    pub version: String,
    pub extensions: Vec<PluginExtension>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHealth {
    pub ready: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginProfileValidation {
    pub valid: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGameCandidate {
    pub provider_id: String,
    pub external_id: String,
    pub title: String,
    pub sort_title: Option<String>,
    pub platform: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDiscoveryPage {
    pub games: Vec<PluginGameCandidate>,
    pub next_cursor: Option<String>,
    pub complete: bool,
}

/// The closed launch mode. WIT carries `mode` as a string; mapping it to this
/// enum is what keeps it from ever becoming a process argument. Adding a mode
/// is an ABI decision, not a plugin's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLaunchMode {
    Default,
}

/// The host's launch intent, and the only thing `prepare-launch` can produce.
/// It holds no executable, no working directory and no arguments: the host
/// resolves those from the profile the user created, exactly as the native Wine
/// adapter does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLaunchIntent {
    runner_id: String,
    profile_id: String,
    game_reference: String,
    mode: PluginLaunchMode,
}

#[allow(dead_code)]
impl PluginLaunchIntent {
    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn game_reference(&self) -> &str {
        &self.game_reference
    }

    pub fn mode(&self) -> PluginLaunchMode {
        self.mode
    }
}

/// What the host asks a component to do. Each variant names the operation *and*
/// the arguments the host will check the answer against, so validation cannot
/// drift from the call.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PluginRequest {
    Identity,
    HealthCheck,
    ValidateProfile {
        profile_id: String,
        display_name: String,
    },
    DiscoverPage {
        profile_id: String,
        cursor: Option<String>,
        limit: u32,
    },
    PrepareLaunch {
        profile_id: String,
        game_reference: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginResponse {
    Identity(PluginIdentity),
    Health(PluginHealth),
    ProfileValidation(PluginProfileValidation),
    DiscoveryPage(PluginDiscoveryPage),
    LaunchIntent(PluginLaunchIntent),
}

impl PluginRequest {
    /// Three tiers, from tightest to loosest: a probe answers about itself, an
    /// interactive call answers about one profile or game, and discovery walks a
    /// bounded page. Nothing here is a background process.
    fn budget(&self, limits: &PluginLimits) -> (u64, Duration) {
        match self {
            Self::Identity | Self::HealthCheck => (limits.probe_fuel, limits.probe_deadline),
            Self::DiscoverPage { .. } => (limits.discovery_fuel, limits.discovery_deadline),
            Self::ValidateProfile { .. } | Self::PrepareLaunch { .. } => {
                (limits.interactive_fuel, limits.interactive_deadline)
            }
        }
    }

    fn decision(&self) -> &'static str {
        match self {
            Self::Identity => "get-identity",
            Self::HealthCheck => "health-check",
            Self::ValidateProfile { .. } => "validate-profile",
            Self::DiscoverPage { .. } => "discover-page",
            Self::PrepareLaunch { .. } => "prepare-launch",
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// A compiled component, kept so a second call does not pay for compilation
/// again. Holding the hash beside it means a cached component can still be
/// matched against the bytes the registry verified.
#[derive(Clone)]
pub struct PreparedComponent {
    component: Component,
    sha256: String,
}

impl std::fmt::Debug for PreparedComponent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedComponent")
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

impl PreparedComponent {
    #[allow(dead_code)]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// What a completed invocation cost. Reported so the plan's performance
/// contract can be checked with numbers rather than adjectives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationCost {
    pub instantiation: Duration,
    pub call: Duration,
    pub fuel_used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInvocation {
    pub response: PluginResponse,
    pub cost: InvocationCost,
}

struct RuntimeInner {
    engine: Engine,
    limits: PluginLimits,
    scheduler_limits: SchedulerLimits,
    budget: Arc<MemoryBudget>,
    journal: Arc<PluginJournal>,
    ticker: Arc<EpochTicker>,
    epoch_mode: EpochMode,
    /// Created on first use, in whichever worker asks for it. A session that
    /// never opens a plugin surface never pays for the worker threads.
    scheduler: OnceLock<Arc<PluginScheduler>>,
}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        self.ticker.stop();
    }
}

/// Cheap to clone, because a queued job has to carry the host that will run it.
/// Everything shared — the engine's compilation cache, the memory ceiling, the
/// journal, the epoch ticker — is shared by every clone, which is what makes
/// those limits global rather than per-caller.
#[derive(Clone)]
pub struct PluginRuntime {
    inner: Arc<RuntimeInner>,
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRuntime")
            .field("limits", &self.inner.limits)
            .field("epoch_mode", &self.inner.epoch_mode)
            .finish_non_exhaustive()
    }
}

impl PluginRuntime {
    /// Runtime creation happens in a worker after the user opens a plugin
    /// surface. It is not part of app startup or rail navigation.
    pub fn new() -> Result<Self, PluginRuntimeError> {
        Self::with_limits(PluginLimits::default(), EpochMode::Threaded)
    }

    pub fn with_limits(
        limits: PluginLimits,
        epoch_mode: EpochMode,
    ) -> Result<Self, PluginRuntimeError> {
        Self::with_all_limits(limits, SchedulerLimits::default(), epoch_mode)
    }

    pub fn with_all_limits(
        limits: PluginLimits,
        scheduler_limits: SchedulerLimits,
        epoch_mode: EpochMode,
    ) -> Result<Self, PluginRuntimeError> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(PLUGIN_WASM_STACK_BYTES)
            .cranelift_opt_level(OptLevel::SpeedAndSize);
        let engine = Engine::new(&config).map_err(|_| PluginRuntimeError::EngineUnavailable)?;
        let ticker = EpochTicker::new(engine.clone(), limits.epoch_tick);
        if epoch_mode == EpochMode::Threaded {
            ticker.spawn();
        }
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                engine,
                limits,
                scheduler_limits,
                budget: Arc::new(MemoryBudget::default()),
                journal: Arc::new(PluginJournal::default()),
                ticker,
                epoch_mode,
                scheduler: OnceLock::new(),
            }),
        })
    }

    /// The process-wide runtime. One `Engine` means one compilation cache and
    /// one memory ceiling for every plugin surface; creating a fresh engine per
    /// command would give each of them their own budget.
    pub fn shared() -> Result<Self, PluginRuntimeError> {
        static SHARED: OnceLock<Option<PluginRuntime>> = OnceLock::new();
        SHARED
            .get_or_init(|| PluginRuntime::new().ok())
            .clone()
            .ok_or(PluginRuntimeError::EngineUnavailable)
    }

    #[allow(dead_code)]
    pub fn journal(&self) -> &Arc<PluginJournal> {
        &self.inner.journal
    }

    #[allow(dead_code)]
    pub fn limits(&self) -> PluginLimits {
        self.inner.limits
    }

    pub fn scheduler(&self) -> &Arc<PluginScheduler> {
        self.inner.scheduler.get_or_init(|| {
            Arc::new(PluginScheduler::new(
                self.inner.scheduler_limits,
                Arc::clone(&self.inner.journal),
            ))
        })
    }

    /// Queues one invocation. This is the door every caller outside this module
    /// should use: it is the only one that cannot run on the rendering path,
    /// cannot exceed the global concurrency, and hands the job's cancel token to
    /// the host so cancelling the job cancels the component.
    pub fn submit(
        &self,
        prepared: &PreparedComponent,
        plugin_id: &str,
        grants: &PluginGrants,
        request: PluginRequest,
    ) -> Result<JobHandle<PluginInvocation>, SubmitError> {
        let runtime = self.clone();
        let prepared = prepared.clone();
        let grants = grants.clone();
        let plugin = plugin_id.to_owned();
        self.scheduler().submit(plugin_id, move |context| {
            runtime.invoke(
                &prepared,
                &plugin,
                &grants,
                context.correlation_id(),
                context.cancel_token(),
                &request,
            )
        })
    }

    /// The check the registry runs before Orivo offers an installed runner for
    /// configuration. It is the one place where a package's three accounts of
    /// itself are made to agree: its manifest, its component type, and what the
    /// component says when asked.
    ///
    /// It runs under the probe budget with the manifest's declared capabilities
    /// and none of the user's grants, so the component has no authority while it
    /// is being identified.
    pub fn verify_runner(
        &self,
        prepared: &PreparedComponent,
        manifest: &ValidatedPluginManifest,
        check: RunnerCheck,
    ) -> Result<Option<PluginHealth>, PluginRuntimeError> {
        let contract = self.inspect_contract(prepared)?;
        if let Some(undeclared) = contract
            .required_capabilities
            .iter()
            .find(|capability| !manifest.declares(**capability))
        {
            return Err(PluginRuntimeError::CapabilityUndeclared(*undeclared));
        }
        if check == RunnerCheck::ContractOnly {
            return Ok(None);
        }

        let grants = PluginGrants::declared_only(manifest);
        let PluginResponse::Identity(identity) =
            self.probe(prepared, manifest, &grants, PluginRequest::Identity)?
        else {
            return Err(PluginRuntimeError::InvalidResult("identity"));
        };
        // A component that reports another id, another version, or an extension
        // its package never announced is not the thing the user consented to
        // install, whatever the signature on the archive said.
        if identity.id != manifest.id()
            || identity.version != manifest.manifest().version
            || !identity
                .extensions
                .iter()
                .all(|extension| manifest.manifest().extensions.contains(extension))
        {
            self.inner.journal.record(
                next_correlation_id(),
                manifest.id(),
                "identity-mismatch",
                "the component disagrees with its manifest",
            );
            return Err(PluginRuntimeError::IdentityMismatch);
        }

        let PluginResponse::Health(health) =
            self.probe(prepared, manifest, &grants, PluginRequest::HealthCheck)?
        else {
            return Err(PluginRuntimeError::InvalidResult("health"));
        };
        Ok(Some(health))
    }

    /// One probe, through the scheduler, with a wall-clock bound of its own.
    ///
    /// The epoch deadline already stops the component, but a queued job can also
    /// be waiting behind another plugin's work. Bounding the wait — and
    /// cancelling on the way out — is what keeps a panel opening from depending
    /// on how busy the worker happens to be.
    fn probe(
        &self,
        prepared: &PreparedComponent,
        manifest: &ValidatedPluginManifest,
        grants: &PluginGrants,
        request: PluginRequest,
    ) -> Result<PluginResponse, PluginRuntimeError> {
        let handle = self
            .submit(prepared, manifest.id(), grants, request)
            .map_err(|error| match error {
                SubmitError::Degraded { .. } => PluginRuntimeError::Paused,
                SubmitError::Busy { .. } => PluginRuntimeError::Busy,
                SubmitError::ShuttingDown => PluginRuntimeError::EngineUnavailable,
            })?;
        let budget = self
            .inner
            .limits
            .probe_deadline
            .saturating_mul(PROBE_WAIT_MULTIPLIER);
        match handle.wait_for(budget) {
            Ok(Ok(invocation)) => Ok(invocation.response),
            Ok(Err(JobError::Cancelled)) => Err(PluginRuntimeError::Cancelled),
            Ok(Err(JobError::Runtime(error))) => Err(error),
            Ok(Err(JobError::Abandoned)) => Err(PluginRuntimeError::EngineUnavailable),
            Err(handle) => {
                handle.cancel();
                Err(PluginRuntimeError::DeadlineExceeded)
            }
        }
    }

    /// Advances the epoch by hand. Only useful with [`EpochMode::Manual`],
    /// where a test drives a deadline instead of waiting for one.
    #[allow(dead_code)]
    pub fn tick_epoch(&self) {
        self.inner.engine.increment_epoch();
    }

    /// Compilation validates every nested core module in a component. No guest
    /// code runs here, so fuel is not consumed and no grant is needed.
    pub fn preflight_component(&self, bytes: &[u8]) -> Result<(), PluginRuntimeError> {
        Component::new(&self.inner.engine, bytes)
            .map(|_| ())
            .map_err(|_| PluginRuntimeError::InvalidComponent)
    }

    /// Compiles once and keeps the result. Preparing ahead of a first call is
    /// the difference the plan asks to measure; `invoke` accepts the prepared
    /// component so the cost is paid where it is visible.
    pub fn prepare_component(
        &self,
        bytes: &[u8],
        sha256: &str,
    ) -> Result<PreparedComponent, PluginRuntimeError> {
        Component::new(&self.inner.engine, bytes)
            .map(|component| PreparedComponent {
                component,
                sha256: sha256.to_owned(),
            })
            .map_err(|_| PluginRuntimeError::InvalidComponent)
    }

    /// One call into a component, under grants and limits, from start to
    /// validated result.
    ///
    /// The store is built, used and dropped here. A component therefore keeps
    /// nothing between calls — no cached handle, no open directory, no linear
    /// memory — which is what makes a per-invocation fuel and memory budget
    /// meaningful instead of cumulative.
    pub fn invoke(
        &self,
        prepared: &PreparedComponent,
        plugin_id: &str,
        grants: &PluginGrants,
        correlation_id: CorrelationId,
        cancel: &Arc<AtomicBool>,
        request: &PluginRequest,
    ) -> Result<PluginInvocation, PluginRuntimeError> {
        let (fuel, deadline) = request.budget(&self.inner.limits);
        let ticks = self.inner.limits.ticks(deadline);

        let mut store = Store::new(
            &self.inner.engine,
            HostState {
                plugin_id: plugin_id.to_owned(),
                correlation_id,
                grants: grants.clone(),
                journal: Arc::clone(&self.inner.journal),
                memory: StoreMemoryGuard {
                    limits: self.inner.limits,
                    budget: Arc::clone(&self.inner.budget),
                    charged: 0,
                    hit_limit: false,
                },
                cancel: Arc::clone(cancel),
                ticks_remaining: ticks,
                interruption: None,
                host_calls: 0,
                bytes_read: 0,
            },
        );
        store.limiter(|state| &mut state.memory);
        store
            .set_fuel(fuel)
            .map_err(|_| PluginRuntimeError::EngineUnavailable)?;
        // One tick at a time, with the budget counted in the callback: the same
        // mechanism then serves both the hard deadline and cancellation, and a
        // cancelled call does not have to wait out the deadline it was given.
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|mut context| {
            let state = context.data_mut();
            if state.cancel.load(Ordering::Relaxed) {
                state.interruption = Some(Interruption::Cancelled);
                return Ok(UpdateDeadline::Interrupt);
            }
            if state.ticks_remaining == 0 {
                state.interruption = Some(Interruption::Deadline);
                return Ok(UpdateDeadline::Interrupt);
            }
            state.ticks_remaining -= 1;
            Ok(UpdateDeadline::Continue(1))
        });

        let linker = self.linker(plugin_id, grants, correlation_id)?;

        let _lease = self.inner.ticker.lease();

        // A cancel that arrives before the first instruction must not be spent
        // instantiating. The epoch callback only runs once wasm is executing, so
        // the check belongs here too.
        if cancel.load(Ordering::Relaxed) {
            let cancelled = PluginRuntimeError::Cancelled;
            return Err(self.finish(&mut store, plugin_id, request, cancelled));
        }

        let started = Instant::now();
        // An import the manifest never declared has no definition here, so this
        // is where a component asking for an undeclared capability stops. The
        // distinction matters to the user: "this package asks for more than it
        // told you" is a different sentence from "this package is broken".
        let instance_pre = match linker.instantiate_pre(&prepared.component) {
            Ok(instance_pre) => instance_pre,
            Err(_) => {
                let error = self.instantiation_refusal(prepared, grants);
                return Err(self.finish(&mut store, plugin_id, request, error));
            }
        };
        let bindings = match RunnerPluginPre::new(instance_pre) {
            Ok(pre) => match pre.instantiate(&mut store) {
                Ok(bindings) => bindings,
                Err(error) => {
                    let mapped = self.classify(&mut store, error);
                    return Err(self.finish(&mut store, plugin_id, request, mapped));
                }
            },
            // The component compiled and its imports were satisfiable, but it
            // does not export the runner world. Nothing here can invoke it.
            Err(_) => {
                let missing = PluginRuntimeError::MissingWorld;
                return Err(self.finish(&mut store, plugin_id, request, missing));
            }
        };
        let instantiation = started.elapsed();

        let called = Instant::now();
        let outcome = self.call(&mut store, &bindings, request);
        let call = called.elapsed();
        let fuel_used = fuel.saturating_sub(store.get_fuel().unwrap_or(0));

        match outcome {
            Ok(response) => {
                self.inner.journal.record(
                    correlation_id,
                    plugin_id,
                    request.decision(),
                    format!(
                        "ok in {}ms, {fuel_used} fuel",
                        instantiation.saturating_add(call).as_millis()
                    ),
                );
                Ok(PluginInvocation {
                    response,
                    cost: InvocationCost {
                        instantiation,
                        call,
                        fuel_used,
                    },
                })
            }
            Err(error) => Err(self.finish(&mut store, plugin_id, request, error)),
        }
    }

    /// Only the imports this grant set allows. Nothing else is added, so a
    /// component that needs more cannot be instantiated — the strongest form of
    /// fail-closed available, because the guest never starts.
    fn linker(
        &self,
        plugin_id: &str,
        grants: &PluginGrants,
        correlation_id: CorrelationId,
    ) -> Result<Linker<HostState>, PluginRuntimeError> {
        let mut linker = Linker::new(&self.inner.engine);
        // The journal is not a capability: it only lets a plugin describe
        // itself, and a refusal is worth nothing if the attempt is invisible.
        host_journal::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|_| PluginRuntimeError::Instantiation)?;
        if grants.declares(PluginCapability::FilesRead) {
            // Linked, but not therefore allowed: `granted_directory` refuses
            // every call until the user has granted a scope.
            host_files::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
                .map_err(|_| PluginRuntimeError::Instantiation)?;
        } else {
            self.inner.journal.record(
                correlation_id,
                plugin_id,
                "capability-unlinked",
                "files_read is not declared, so its host import is absent",
            );
        }
        Ok(linker)
    }

    /// What the component's own type says it needs, without running it and
    /// without believing its manifest.
    ///
    /// This is the check that keeps an installed package from being offered for
    /// configuration when it cannot work: it either implements the runner world
    /// and imports only capabilities Orivo can serve, or it does not.
    pub fn inspect_contract(
        &self,
        prepared: &PreparedComponent,
    ) -> Result<ComponentContract, PluginRuntimeError> {
        let component_type = prepared.component.component_type();
        let mut required_capabilities = BTreeSet::new();
        for (name, item) in component_type.imports(&self.inner.engine) {
            match name {
                HOST_JOURNAL_IMPORT => {}
                TYPES_IMPORT if !self.imports_any_function(&item) => {}
                HOST_FILES_IMPORT => {
                    required_capabilities.insert(PluginCapability::FilesRead);
                }
                _ => return Err(PluginRuntimeError::UnknownImport),
            }
        }
        let exports = component_type
            .exports(&self.inner.engine)
            .map(|(name, _)| name)
            .collect::<BTreeSet<_>>();
        if !exports.contains(PLUGIN_CORE_EXPORT) || !exports.contains(RUNNER_EXPORT) {
            return Err(PluginRuntimeError::MissingWorld);
        }
        Ok(ComponentContract {
            required_capabilities,
        })
    }

    fn imports_any_function(&self, item: &ComponentItem) -> bool {
        match item {
            ComponentItem::ComponentInstance(instance) => instance
                .exports(&self.inner.engine)
                .any(|(_, item)| matches!(item, ComponentItem::ComponentFunc(_))),
            ComponentItem::ComponentFunc(_) => true,
            _ => false,
        }
    }

    /// Why instantiation was refused, in the user's terms. Asking the
    /// component's own type is how the host tells "this package is broken" from
    /// "this plugin needs a folder you have not allowed".
    fn instantiation_refusal(
        &self,
        prepared: &PreparedComponent,
        grants: &PluginGrants,
    ) -> PluginRuntimeError {
        match self.inspect_contract(prepared) {
            Err(error) => error,
            Ok(contract) => contract
                .required_capabilities
                .into_iter()
                .find(|capability| !grants.declares(*capability))
                .map(PluginRuntimeError::CapabilityUndeclared)
                .unwrap_or(PluginRuntimeError::Instantiation),
        }
    }

    fn call(
        &self,
        store: &mut Store<HostState>,
        bindings: &RunnerPlugin,
        request: &PluginRequest,
    ) -> Result<PluginResponse, PluginRuntimeError> {
        match request {
            PluginRequest::Identity => {
                let identity = bindings
                    .orivo_plugin_plugin_core()
                    .call_get_identity(&mut *store)
                    .map_err(|error| self.classify(store, error))?;
                validate_identity(identity).map(PluginResponse::Identity)
            }
            PluginRequest::HealthCheck => {
                let health = bindings
                    .orivo_plugin_plugin_core()
                    .call_health_check(&mut *store)
                    .map_err(|error| self.classify(store, error))??;
                validate_health(health).map(PluginResponse::Health)
            }
            PluginRequest::ValidateProfile {
                profile_id,
                display_name,
            } => {
                let profile = wit_runner::RunnerProfile {
                    id: profile_id.clone(),
                    display_name: display_name.clone(),
                };
                let validation = bindings
                    .orivo_plugin_runner()
                    .call_validate_profile(&mut *store, &profile)
                    .map_err(|error| self.classify(store, error))??;
                validate_profile_validation(validation).map(PluginResponse::ProfileValidation)
            }
            PluginRequest::DiscoverPage {
                profile_id,
                cursor,
                limit,
            } => {
                let page_request = wit_types::PageRequest {
                    cursor: cursor.clone(),
                    limit: *limit,
                };
                let page = bindings
                    .orivo_plugin_runner()
                    .call_discover_page(&mut *store, profile_id, &page_request)
                    .map_err(|error| self.classify(store, error))??;
                validate_discovery_page(page, *limit).map(PluginResponse::DiscoveryPage)
            }
            PluginRequest::PrepareLaunch {
                profile_id,
                game_reference,
            } => {
                let intent = bindings
                    .orivo_plugin_runner()
                    .call_prepare_launch(&mut *store, profile_id, game_reference)
                    .map_err(|error| self.classify(store, error))??;
                validate_launch_intent(intent, profile_id, game_reference)
                    .map(PluginResponse::LaunchIntent)
            }
        }
    }

    /// Turns a Wasmtime error into the host's own vocabulary. The order matters:
    /// Wasmtime reports a blown deadline and a cancellation identically, and a
    /// guest allocator that aborts after a refused `memory.grow` looks like any
    /// other trap, so the store's own record of what the host did is consulted
    /// before the trap code.
    fn classify(&self, store: &mut Store<HostState>, error: wasmtime::Error) -> PluginRuntimeError {
        let state = store.data();
        if let Some(interruption) = state.interruption {
            return match interruption {
                Interruption::Cancelled => PluginRuntimeError::Cancelled,
                Interruption::Deadline => PluginRuntimeError::DeadlineExceeded,
            };
        }
        if state.memory.hit_limit {
            return PluginRuntimeError::MemoryLimit;
        }
        match error.downcast_ref::<Trap>() {
            Some(Trap::OutOfFuel) => PluginRuntimeError::FuelExhausted,
            Some(Trap::Interrupt) => PluginRuntimeError::DeadlineExceeded,
            Some(_) => PluginRuntimeError::Trapped,
            None => PluginRuntimeError::Trapped,
        }
    }

    /// Journals a refusal and hands it back, so no failure path can return
    /// without the reason for it being recorded next to its correlation id.
    fn finish(
        &self,
        store: &mut Store<HostState>,
        plugin_id: &str,
        request: &PluginRequest,
        error: PluginRuntimeError,
    ) -> PluginRuntimeError {
        self.inner.journal.record(
            store.data().correlation_id,
            plugin_id,
            request.decision(),
            format!("refused: {error}"),
        );
        error
    }
}

// ---------------------------------------------------------------------------
// Result validation
// ---------------------------------------------------------------------------

/// Guest text, or nothing. Control characters are removed rather than escaped
/// because this string can end up in a view model, and an over-long or
/// invisible-character title is a presentation bug the host can simply refuse.
fn sanitise_text(value: &str, max_bytes: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_bytes || trimmed.chars().any(char::is_control) {
        return None;
    }
    Some(trimmed.to_owned())
}

fn optional_text(
    value: Option<String>,
    max_bytes: usize,
    reason: &'static str,
) -> Result<Option<String>, PluginRuntimeError> {
    match value {
        None => Ok(None),
        Some(text) => sanitise_text(&text, max_bytes)
            .map(Some)
            .ok_or(PluginRuntimeError::InvalidResult(reason)),
    }
}

fn validate_identity(
    identity: wit_core::Identity,
) -> Result<PluginIdentity, PluginRuntimeError> {
    if !valid_opaque_id(&identity.id, MAX_RESULT_ID_BYTES) {
        return Err(PluginRuntimeError::InvalidResult("identity id"));
    }
    let version = sanitise_text(&identity.version, 32)
        .ok_or(PluginRuntimeError::InvalidResult("identity version"))?;
    let mut extensions = Vec::new();
    for kind in identity.extensions {
        extensions.push(match kind {
            wit_types::ExtensionKind::Source => PluginExtension::Source,
            wit_types::ExtensionKind::Runner => PluginExtension::Runner,
            wit_types::ExtensionKind::Metadata => PluginExtension::Metadata,
            wit_types::ExtensionKind::Search => PluginExtension::Search,
            wit_types::ExtensionKind::Automation => PluginExtension::Automation,
            wit_types::ExtensionKind::UiContribution => PluginExtension::UiContribution,
        });
    }
    extensions.sort();
    extensions.dedup();
    Ok(PluginIdentity {
        id: identity.id,
        version,
        extensions,
    })
}

fn validate_health(
    health: wit_core::Health,
) -> Result<PluginHealth, PluginRuntimeError> {
    Ok(PluginHealth {
        ready: health.ready,
        message: optional_text(health.message, MAX_RESULT_TEXT_BYTES, "health message")?,
    })
}

fn validate_profile_validation(
    validation: wit_runner::ProfileValidation,
) -> Result<PluginProfileValidation, PluginRuntimeError> {
    Ok(PluginProfileValidation {
        valid: validation.valid,
        message: optional_text(validation.message, MAX_RESULT_TEXT_BYTES, "profile message")?,
    })
}

fn validate_discovery_page(
    page: wit_runner::RunnerGamePage,
    limit: u32,
) -> Result<PluginDiscoveryPage, PluginRuntimeError> {
    let ceiling = (limit as usize).min(MAX_RESULT_PAGE_GAMES);
    if page.games.len() > ceiling {
        return Err(PluginRuntimeError::InvalidResult("page longer than asked"));
    }
    let mut games = Vec::with_capacity(page.games.len());
    let mut seen = BTreeSet::new();
    for candidate in page.games {
        if !valid_opaque_id(&candidate.reference.provider_id, MAX_RESULT_ID_BYTES)
            || !valid_opaque_id(&candidate.reference.external_id, MAX_RESULT_ID_BYTES)
        {
            return Err(PluginRuntimeError::InvalidResult("candidate reference"));
        }
        // A duplicate external reference would make an idempotent import write
        // the same game twice, so it is rejected before anything is committed.
        if !seen.insert((
            candidate.reference.provider_id.clone(),
            candidate.reference.external_id.clone(),
        )) {
            return Err(PluginRuntimeError::InvalidResult("duplicate reference"));
        }
        let title = sanitise_text(&candidate.title, MAX_RESULT_TEXT_BYTES)
            .ok_or(PluginRuntimeError::InvalidResult("candidate title"))?;
        games.push(PluginGameCandidate {
            provider_id: candidate.reference.provider_id,
            external_id: candidate.reference.external_id,
            title,
            sort_title: optional_text(
                candidate.sort_title,
                MAX_RESULT_TEXT_BYTES,
                "candidate sort title",
            )?,
            platform: optional_text(
                candidate.platform,
                MAX_RESULT_TEXT_BYTES,
                "candidate platform",
            )?,
            installed: candidate.installed,
        });
    }
    let next_cursor = match page.page.next_cursor {
        None => None,
        Some(cursor) if valid_opaque_id(&cursor, MAX_RESULT_CURSOR_BYTES) => Some(cursor),
        Some(_) => return Err(PluginRuntimeError::InvalidResult("page cursor")),
    };
    // A page that says it is complete but hands back a cursor would leave an
    // import unable to decide whether to resume.
    if page.page.complete && next_cursor.is_some() {
        return Err(PluginRuntimeError::InvalidResult("cursor after completion"));
    }
    Ok(PluginDiscoveryPage {
        games,
        next_cursor,
        complete: page.page.complete,
    })
}

/// The intent must describe the call the host just made. A plugin that answers
/// about another profile or another game is not preparing a launch, it is
/// proposing one, and the host does not accept proposals here.
fn validate_launch_intent(
    intent: wit_runner::LaunchIntent,
    profile_id: &str,
    game_reference: &str,
) -> Result<PluginLaunchIntent, PluginRuntimeError> {
    if intent.profile_id != profile_id {
        return Err(PluginRuntimeError::InvalidResult("intent profile"));
    }
    if intent.game_reference != game_reference {
        return Err(PluginRuntimeError::InvalidResult("intent game reference"));
    }
    if !valid_opaque_id(&intent.runner_id, MAX_RESULT_ID_BYTES) {
        return Err(PluginRuntimeError::InvalidResult("intent runner"));
    }
    let mode = match intent.mode.as_str() {
        "default" => PluginLaunchMode::Default,
        _ => return Err(PluginRuntimeError::InvalidResult("intent mode")),
    };
    Ok(PluginLaunchIntent {
        runner_id: intent.runner_id,
        profile_id: intent.profile_id,
        game_reference: intent.game_reference,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_manifest::{
        ArtifactDescriptor, ArtifactKind, PLUGIN_SDK_V1, PluginManifest,
    };
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Minimal empty Component Model binary: wasm magic, component version,
    // and an empty payload. It is enough to exercise Wasmtime's component
    // validator without executing untrusted guest code.
    const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    /// The reference runner from `src-tauri/fixtures/runner-fixture`. It is
    /// committed rather than built here so `cargo test` needs no WebAssembly
    /// target and no component tool; `build.sh` beside it is how it changes.
    const FIXTURE: &[u8] = include_bytes!("../fixtures/orivo-runner-fixture.wasm");
    /// Regenerated by `build.sh`, which prints this digest.
    const FIXTURE_SHA256: &str =
        "3244b7304cf85d8e8a52151174d9116011f51154c586350e0f80c70e252608a8";
    /// A component whose only import is WASI. Also built by `build.sh`, from
    /// hand-written component text rather than a second Rust guest.
    const WASI_IMPORT: &[u8] = include_bytes!("../fixtures/wasi-import.wasm");
    const WASI_IMPORT_SHA256: &str =
        "4b7909ec90668f639b6023c4b844a8bf08201fda44125b68acc44f24b7b12630";

    const FIXTURE_PLUGIN_ID: &str = "com.orivo.fixture-runner";
    const FIXTURE_PROFILE: &str = "fixture-profile-1";
    const GAMES_GRANT: &str = "fixture-games";

    fn temporary_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "orivo-plugin-host-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// A fixture library, plus a file just outside it. The escape test is only
    /// meaningful if there is something to reach.
    struct FixtureLibrary {
        root: PathBuf,
        games: PathBuf,
    }

    impl FixtureLibrary {
        fn new(tag: &str) -> Self {
            let root = temporary_root(tag);
            let games = root.join("games");
            fs::create_dir_all(&games).unwrap();
            fs::write(games.join("alpha.rom"), b"Alpha Quest\n").unwrap();
            fs::write(games.join("beta.rom"), b"Beta Racer\n").unwrap();
            fs::write(games.join("gamma.rom"), b"Gamma Tactics\n").unwrap();
            fs::write(games.join("readme.txt"), b"not a rom").unwrap();
            fs::write(root.join("secret.txt"), b"a keychain token").unwrap();
            Self { root, games }
        }

        fn directories(&self) -> BTreeMap<String, PathBuf> {
            BTreeMap::from([(GAMES_GRANT.to_string(), self.games.clone())])
        }
    }

    impl Drop for FixtureLibrary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture_manifest(capabilities: Vec<PluginCapability>) -> ValidatedPluginManifest {
        PluginManifest {
            id: FIXTURE_PLUGIN_ID.into(),
            name: "Fixture Runner".into(),
            version: "1.0.0".into(),
            sdk: PLUGIN_SDK_V1.into(),
            min_orivo_version: Some("0.3.0".into()),
            extensions: vec![PluginExtension::Runner],
            capabilities,
            network_domains: Vec::new(),
            artifacts: vec![ArtifactDescriptor {
                path: "component.wasm".into(),
                kind: ArtifactKind::Component,
                sha256: FIXTURE_SHA256.into(),
                byte_size: FIXTURE.len() as u64,
            }],
        }
        .validate()
        .unwrap()
    }

    fn files_grant(ids: &[&str]) -> CapabilityGrant {
        CapabilityGrant {
            plugin_id: FIXTURE_PLUGIN_ID.into(),
            capability: PluginCapability::FilesRead,
            scope: CapabilityScope::DirectoryGrants(
                ids.iter().map(|id| (*id).to_string()).collect(),
            ),
        }
    }

    /// Everything a host test needs: a runtime with the limits under test, the
    /// prepared fixture, and the grants in force.
    struct Harness {
        runtime: PluginRuntime,
        prepared: PreparedComponent,
        grants: PluginGrants,
    }

    impl Harness {
        fn new(limits: PluginLimits, library: Option<&FixtureLibrary>) -> Self {
            let runtime = PluginRuntime::with_limits(limits, EpochMode::Threaded).unwrap();
            let prepared = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
            let grants = match library {
                Some(library) => PluginGrants::resolve(
                    &fixture_manifest(vec![
                        PluginCapability::RunnerPrepare,
                        PluginCapability::FilesRead,
                    ]),
                    &[files_grant(&[GAMES_GRANT])],
                    &library.directories(),
                )
                .unwrap(),
                None => PluginGrants::none(),
            };
            Self {
                runtime,
                prepared,
                grants,
            }
        }

        fn call(&self, request: PluginRequest) -> Result<PluginResponse, PluginRuntimeError> {
            self.call_with(request, &Arc::new(AtomicBool::new(false)))
                .map(|invocation| invocation.response)
        }

        fn call_with(
            &self,
            request: PluginRequest,
            cancel: &Arc<AtomicBool>,
        ) -> Result<PluginInvocation, PluginRuntimeError> {
            self.runtime.invoke(
                &self.prepared,
                FIXTURE_PLUGIN_ID,
                &self.grants,
                next_correlation_id(),
                cancel,
                &request,
            )
        }

        fn prepare(&self, game_reference: &str) -> Result<PluginResponse, PluginRuntimeError> {
            self.call(PluginRequest::PrepareLaunch {
                profile_id: FIXTURE_PROFILE.into(),
                game_reference: game_reference.into(),
            })
        }
    }

    // -----------------------------------------------------------------------
    // The contract itself
    // -----------------------------------------------------------------------

    #[test]
    fn sdk_wit_contract_is_parseable() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wit");
        wit_parser::Resolve::default().push_dir(path).unwrap();
    }

    #[test]
    fn accepts_a_component_and_rejects_non_wasm_bytes() {
        let runtime = PluginRuntime::new().unwrap();
        assert!(runtime.preflight_component(EMPTY_COMPONENT).is_ok());
        assert_eq!(
            runtime.preflight_component(b"not a component").unwrap_err(),
            PluginRuntimeError::InvalidComponent
        );
    }

    /// The committed artefact is the test input. If it drifts from the source
    /// beside it, every sandbox assertion below is testing something else.
    #[test]
    fn the_committed_fixture_matches_its_recorded_digest() {
        let mut digest = Sha256::new();
        digest.update(FIXTURE);
        assert_eq!(format!("{:x}", digest.finalize()), FIXTURE_SHA256);
    }

    #[test]
    fn a_component_without_the_runner_world_is_not_invoked() {
        let runtime = PluginRuntime::new().unwrap();
        let prepared = runtime.prepare_component(EMPTY_COMPONENT, "0".repeat(64).as_str()).unwrap();
        let error = runtime
            .invoke(
                &prepared,
                FIXTURE_PLUGIN_ID,
                &PluginGrants::none(),
                next_correlation_id(),
                &Arc::new(AtomicBool::new(false)),
                &PluginRequest::Identity,
            )
            .unwrap_err();
        assert_eq!(error, PluginRuntimeError::MissingWorld);
    }

    /// The contract check is what the registry runs before it offers a package
    /// for configuration, and it runs no guest code.
    #[test]
    fn the_contract_check_reads_a_components_own_type() {
        let runtime = PluginRuntime::new().unwrap();

        let fixture = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
        assert_eq!(
            runtime.inspect_contract(&fixture).unwrap(),
            ComponentContract {
                required_capabilities: BTreeSet::from([PluginCapability::FilesRead]),
            }
        );

        let empty = runtime
            .prepare_component(EMPTY_COMPONENT, "0".repeat(64).as_str())
            .unwrap();
        assert_eq!(
            runtime.inspect_contract(&empty).unwrap_err(),
            PluginRuntimeError::MissingWorld
        );
    }

    /// The host provides no WASI. A component that expects a clock, a socket or
    /// a preopened directory has nothing to link against and is refused before a
    /// `Store` exists — whatever its manifest claims.
    /// The contract half of a verification must hold whether or not the pass has
    /// the budget to call the component: a package past the probe bound is still
    /// refused for exporting the wrong world.
    #[test]
    fn a_contract_only_check_still_refuses_a_broken_package() {
        let runtime = PluginRuntime::new().unwrap();
        let manifest = fixture_manifest(vec![
            PluginCapability::RunnerPrepare,
            PluginCapability::FilesRead,
        ]);

        let empty = runtime
            .prepare_component(EMPTY_COMPONENT, "0".repeat(64).as_str())
            .unwrap();
        assert_eq!(
            runtime
                .verify_runner(&empty, &manifest, RunnerCheck::ContractOnly)
                .unwrap_err(),
            PluginRuntimeError::MissingWorld
        );

        // The fixture's contract is sound, so a contract-only check passes it
        // without running a single instruction — hence no health to report.
        let fixture = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
        assert_eq!(
            runtime
                .verify_runner(&fixture, &manifest, RunnerCheck::ContractOnly)
                .unwrap(),
            None
        );
        assert!(
            runtime
                .verify_runner(&fixture, &manifest, RunnerCheck::ContractAndHealth)
                .unwrap()
                .is_some_and(|health| health.ready)
        );
    }

    /// The manifest is the consent screen. A component needing more than it
    /// lists was agreed to under a false description of itself.
    #[test]
    fn a_component_needing_more_than_its_manifest_declares_is_refused() {
        let runtime = PluginRuntime::new().unwrap();
        let prepared = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
        assert_eq!(
            runtime
                .verify_runner(
                    &prepared,
                    &fixture_manifest(vec![PluginCapability::RunnerPrepare]),
                    RunnerCheck::ContractAndHealth,
                )
                .unwrap_err(),
            PluginRuntimeError::CapabilityUndeclared(PluginCapability::FilesRead)
        );
    }

    #[test]
    fn a_component_importing_wasi_is_refused() {
        let mut digest = Sha256::new();
        digest.update(WASI_IMPORT);
        assert_eq!(format!("{:x}", digest.finalize()), WASI_IMPORT_SHA256);

        let runtime = PluginRuntime::new().unwrap();
        let prepared = runtime
            .prepare_component(WASI_IMPORT, WASI_IMPORT_SHA256)
            .unwrap();
        assert_eq!(
            runtime.inspect_contract(&prepared).unwrap_err(),
            PluginRuntimeError::UnknownImport
        );
        assert_eq!(
            runtime
                .invoke(
                    &prepared,
                    FIXTURE_PLUGIN_ID,
                    &PluginGrants::none(),
                    next_correlation_id(),
                    &Arc::new(AtomicBool::new(false)),
                    &PluginRequest::Identity,
                )
                .unwrap_err(),
            PluginRuntimeError::UnknownImport
        );
    }

    // -----------------------------------------------------------------------
    // Nominal invocation
    // -----------------------------------------------------------------------

    #[test]
    fn invokes_every_runner_export_end_to_end() {
        let library = FixtureLibrary::new("nominal");
        let harness = Harness::new(PluginLimits::default(), Some(&library));

        let PluginResponse::Identity(identity) = harness.call(PluginRequest::Identity).unwrap()
        else {
            panic!("expected an identity");
        };
        assert_eq!(identity.id, FIXTURE_PLUGIN_ID);
        assert_eq!(identity.extensions, vec![PluginExtension::Runner]);

        let PluginResponse::Health(health) = harness.call(PluginRequest::HealthCheck).unwrap()
        else {
            panic!("expected health");
        };
        assert!(health.ready);

        let PluginResponse::ProfileValidation(validation) = harness
            .call(PluginRequest::ValidateProfile {
                profile_id: FIXTURE_PROFILE.into(),
                display_name: "Fixture".into(),
            })
            .unwrap()
        else {
            panic!("expected a validation");
        };
        assert!(validation.valid);

        let PluginResponse::DiscoveryPage(page) = harness
            .call(PluginRequest::DiscoverPage {
                profile_id: FIXTURE_PROFILE.into(),
                cursor: None,
                limit: 10,
            })
            .unwrap()
        else {
            panic!("expected a page");
        };
        // The titles come out of the fixture folder, so this also proves the
        // granted directory was really read through the host import.
        assert_eq!(
            page.games
                .iter()
                .map(|game| game.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha Quest", "Beta Racer", "Gamma Tactics"]
        );
        assert!(page.complete);
        assert_eq!(page.next_cursor, None);

        let PluginResponse::LaunchIntent(intent) = harness.prepare("fixture:ok").unwrap() else {
            panic!("expected an intent");
        };
        assert_eq!(intent.runner_id(), FIXTURE_PLUGIN_ID);
        assert_eq!(intent.profile_id(), FIXTURE_PROFILE);
        assert_eq!(intent.game_reference(), "fixture:ok");
        assert_eq!(intent.mode(), PluginLaunchMode::Default);
    }

    #[test]
    fn discovery_resumes_from_its_own_cursor() {
        let library = FixtureLibrary::new("cursor");
        let harness = Harness::new(PluginLimits::default(), Some(&library));

        let PluginResponse::DiscoveryPage(first) = harness
            .call(PluginRequest::DiscoverPage {
                profile_id: FIXTURE_PROFILE.into(),
                cursor: None,
                limit: 2,
            })
            .unwrap()
        else {
            panic!("expected a page");
        };
        assert_eq!(first.games.len(), 2);
        assert!(!first.complete);
        let cursor = first.next_cursor.clone().expect("a cursor");

        let PluginResponse::DiscoveryPage(second) = harness
            .call(PluginRequest::DiscoverPage {
                profile_id: FIXTURE_PROFILE.into(),
                cursor: Some(cursor),
                limit: 2,
            })
            .unwrap()
        else {
            panic!("expected a page");
        };
        assert_eq!(
            second
                .games
                .iter()
                .map(|game| game.external_id.as_str())
                .collect::<Vec<_>>(),
            vec!["gamma"]
        );
        assert!(second.complete);
    }

    // -----------------------------------------------------------------------
    // Grants
    // -----------------------------------------------------------------------

    /// A component asking for a capability its manifest never declared is not
    /// linked to it, so it cannot be instantiated at all.
    #[test]
    fn an_undeclared_capability_refuses_instantiation() {
        let harness = Harness::new(PluginLimits::default(), None);
        let error = harness.call(PluginRequest::Identity).unwrap_err();
        assert_eq!(
            error,
            PluginRuntimeError::CapabilityUndeclared(PluginCapability::FilesRead)
        );
        assert!(
            harness
                .runtime
                .journal()
                .entries()
                .iter()
                .any(|entry| entry.decision == "capability-unlinked")
        );
    }

    /// Declared but not granted is the state every plugin is in between being
    /// installed and being configured. It has to be able to start and describe
    /// itself there, with every capability call refused.
    #[test]
    fn a_declared_but_ungranted_capability_starts_and_refuses() {
        let runtime = PluginRuntime::new().unwrap();
        let prepared = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
        let manifest = fixture_manifest(vec![
            PluginCapability::RunnerPrepare,
            PluginCapability::FilesRead,
        ]);
        let grants = PluginGrants::declared_only(&manifest);
        let cancel = Arc::new(AtomicBool::new(false));

        let PluginResponse::Identity(identity) = runtime
            .invoke(
                &prepared,
                FIXTURE_PLUGIN_ID,
                &grants,
                next_correlation_id(),
                &cancel,
                &PluginRequest::Identity,
            )
            .unwrap()
            .response
        else {
            panic!("expected an identity");
        };
        assert_eq!(identity.id, FIXTURE_PLUGIN_ID);

        let error = runtime
            .invoke(
                &prepared,
                FIXTURE_PLUGIN_ID,
                &grants,
                next_correlation_id(),
                &cancel,
                &PluginRequest::DiscoverPage {
                    profile_id: FIXTURE_PROFILE.into(),
                    cursor: None,
                    limit: 4,
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PluginRuntimeError::Plugin {
                code: PluginErrorCode::PermissionDenied,
                ..
            }
        ));
    }

    #[test]
    fn a_directory_outside_the_grant_is_refused_at_the_call() {
        let library = FixtureLibrary::new("deny");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let error = harness.prepare("fixture:deny").unwrap_err();
        assert!(matches!(
            error,
            PluginRuntimeError::Plugin {
                code: PluginErrorCode::PermissionDenied,
                ..
            }
        ));
        assert!(
            harness
                .runtime
                .journal()
                .entries()
                .iter()
                .any(|entry| entry.decision == "scope-refused")
        );
    }

    #[test]
    fn a_name_that_leaves_the_granted_directory_is_refused() {
        let library = FixtureLibrary::new("escape");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let error = harness.prepare("fixture:escape").unwrap_err();
        assert!(matches!(
            error,
            PluginRuntimeError::Plugin {
                code: PluginErrorCode::InvalidInput,
                ..
            }
        ));
    }

    #[test]
    fn a_symlink_inside_a_granted_directory_is_not_listed() {
        let library = FixtureLibrary::new("symlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink(library.root.join("secret.txt"), library.games.join("zeta.rom"))
            .unwrap();
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let PluginResponse::DiscoveryPage(page) = harness
            .call(PluginRequest::DiscoverPage {
                profile_id: FIXTURE_PROFILE.into(),
                cursor: None,
                limit: 10,
            })
            .unwrap()
        else {
            panic!("expected a page");
        };
        assert!(
            page.games
                .iter()
                .all(|game| game.external_id != "zeta"),
            "a symlink out of the grant was listed"
        );
    }

    #[test]
    fn a_grant_for_an_undeclared_capability_is_refused() {
        let library = FixtureLibrary::new("undeclared");
        let error = PluginGrants::resolve(
            &fixture_manifest(vec![PluginCapability::RunnerPrepare]),
            &[files_grant(&[GAMES_GRANT])],
            &library.directories(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            GrantValidationError::CapabilityNotDeclared(PluginCapability::FilesRead)
        );
    }

    #[test]
    fn a_grant_naming_a_directory_the_host_cannot_resolve_is_refused() {
        let error = PluginGrants::resolve(
            &fixture_manifest(vec![
                PluginCapability::RunnerPrepare,
                PluginCapability::FilesRead,
            ]),
            &[files_grant(&["fixture-gone"])],
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            GrantValidationError::InvalidScope(PluginCapability::FilesRead)
        );
    }

    // -----------------------------------------------------------------------
    // Limits
    // -----------------------------------------------------------------------

    #[test]
    fn fuel_runs_out_before_an_endless_component_does() {
        let library = FixtureLibrary::new("fuel");
        let harness = Harness::new(
            PluginLimits {
                interactive_fuel: 2_000_000,
                interactive_deadline: Duration::from_secs(30),
                ..PluginLimits::default()
            },
            Some(&library),
        );
        assert_eq!(
            harness.prepare("fixture:spin").unwrap_err(),
            PluginRuntimeError::FuelExhausted
        );
    }

    #[test]
    fn the_epoch_deadline_interrupts_an_endless_component() {
        let library = FixtureLibrary::new("deadline");
        let harness = Harness::new(
            PluginLimits {
                // Enough fuel that the deadline is certainly what fires first.
                interactive_fuel: 1 << 42,
                interactive_deadline: Duration::from_millis(40),
                epoch_tick: Duration::from_millis(5),
                ..PluginLimits::default()
            },
            Some(&library),
        );
        let started = Instant::now();
        assert_eq!(
            harness.prepare("fixture:spin").unwrap_err(),
            PluginRuntimeError::DeadlineExceeded
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline did not interrupt the component"
        );
    }

    /// The deadline is counted in epoch ticks, not read off a clock. That is the
    /// seam a test — or a later adversarial suite — uses to decide exactly when a
    /// component runs out of time: with [`EpochMode::Manual`] nothing advances
    /// the epoch except the caller.
    #[test]
    fn the_deadline_is_counted_in_epoch_ticks() {
        let library = FixtureLibrary::new("manual-epoch");
        let limits = PluginLimits {
            interactive_fuel: 1 << 42,
            interactive_deadline: Duration::from_millis(30),
            epoch_tick: Duration::from_millis(10),
            ..PluginLimits::default()
        };
        let runtime = PluginRuntime::with_limits(limits, EpochMode::Manual).unwrap();
        let prepared = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
        let grants = PluginGrants::resolve(
            &fixture_manifest(vec![
                PluginCapability::RunnerPrepare,
                PluginCapability::FilesRead,
            ]),
            &[files_grant(&[GAMES_GRANT])],
            &library.directories(),
        )
        .unwrap();

        let ticker = runtime.clone();
        let ticking = thread::spawn(move || {
            // Three ticks is the budget; the fourth is what the host answers
            // with an interrupt. Nothing else moves the epoch.
            for _ in 0..4 {
                thread::sleep(Duration::from_millis(5));
                ticker.tick_epoch();
            }
        });
        let error = runtime
            .invoke(
                &prepared,
                FIXTURE_PLUGIN_ID,
                &grants,
                next_correlation_id(),
                &Arc::new(AtomicBool::new(false)),
                &PluginRequest::PrepareLaunch {
                    profile_id: FIXTURE_PROFILE.into(),
                    game_reference: "fixture:spin".into(),
                },
            )
            .unwrap_err();
        ticking.join().unwrap();
        assert_eq!(error, PluginRuntimeError::DeadlineExceeded);
    }

    #[test]
    fn a_memory_ceiling_stops_a_growing_component() {
        let library = FixtureLibrary::new("memory");
        let harness = Harness::new(
            PluginLimits {
                instance_memory_bytes: 8 * 1024 * 1024,
                total_memory_bytes: 8 * 1024 * 1024,
                interactive_deadline: Duration::from_secs(30),
                ..PluginLimits::default()
            },
            Some(&library),
        );
        assert_eq!(
            harness.prepare("fixture:grow").unwrap_err(),
            PluginRuntimeError::MemoryLimit
        );
    }

    /// The per-instance ceiling would be meaningless if a second store could
    /// claim the same bytes again.
    #[test]
    fn the_global_ceiling_is_shared_by_every_instance() {
        let budget = MemoryBudget::default();
        assert!(budget.charge(6 * 1024 * 1024, 8 * 1024 * 1024));
        assert!(!budget.charge(6 * 1024 * 1024, 8 * 1024 * 1024));
        budget.release(6 * 1024 * 1024);
        assert!(budget.charge(6 * 1024 * 1024, 8 * 1024 * 1024));
    }

    #[test]
    fn a_cancel_flag_stops_a_call_already_inside_the_component() {
        let library = FixtureLibrary::new("cancel");
        let harness = Harness::new(
            PluginLimits {
                interactive_fuel: 1 << 42,
                interactive_deadline: Duration::from_secs(30),
                epoch_tick: Duration::from_millis(5),
                ..PluginLimits::default()
            },
            Some(&library),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let error = harness
            .call_with(
                PluginRequest::PrepareLaunch {
                    profile_id: FIXTURE_PROFILE.into(),
                    game_reference: "fixture:spin".into(),
                },
                &cancel,
            )
            .unwrap_err();
        assert_eq!(error, PluginRuntimeError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_cancel_before_the_first_instruction_never_instantiates() {
        let library = FixtureLibrary::new("cancel-early");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let cancel = Arc::new(AtomicBool::new(true));
        assert_eq!(
            harness
                .call_with(PluginRequest::Identity, &cancel)
                .unwrap_err(),
            PluginRuntimeError::Cancelled
        );
    }

    /// A cancellation is the user's decision and a refused capability is the
    /// host's. Neither may push a plugin towards `degraded`.
    #[test]
    fn only_a_plugins_own_failures_count_against_it() {
        assert!(!PluginRuntimeError::Cancelled.counts_as_plugin_failure());
        assert!(
            !PluginRuntimeError::CapabilityUndeclared(PluginCapability::FilesRead)
                .counts_as_plugin_failure()
        );
        assert!(PluginRuntimeError::DeadlineExceeded.counts_as_plugin_failure());
        assert!(PluginRuntimeError::InvalidResult("intent mode").counts_as_plugin_failure());
    }

    // -----------------------------------------------------------------------
    // Untrusted results
    // -----------------------------------------------------------------------

    #[test]
    fn the_host_refuses_a_launch_mode_it_does_not_recognise() {
        let library = FixtureLibrary::new("bad-mode");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert_eq!(
            harness.prepare("fixture:bad-mode").unwrap_err(),
            PluginRuntimeError::InvalidResult("intent mode")
        );
    }

    #[test]
    fn the_host_refuses_an_intent_about_another_profile() {
        let library = FixtureLibrary::new("bad-target");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert_eq!(
            harness.prepare("fixture:bad-target").unwrap_err(),
            PluginRuntimeError::InvalidResult("intent profile")
        );
    }

    #[test]
    fn the_host_refuses_a_game_reference_that_is_really_a_path() {
        let library = FixtureLibrary::new("bad-id");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert_eq!(
            harness.prepare("fixture:bad-id").unwrap_err(),
            PluginRuntimeError::InvalidResult("intent game reference")
        );
    }

    #[test]
    fn a_plugin_error_reaches_the_host_as_bounded_text() {
        let library = FixtureLibrary::new("fail");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let error = harness.prepare("fixture:fail").unwrap_err();
        assert_eq!(
            error,
            PluginRuntimeError::Plugin {
                code: PluginErrorCode::Unavailable,
                message: "The fixture runner was asked to fail.".into(),
                retryable: false,
            }
        );
    }

    fn candidate(provider: &str, external: &str, title: &str) -> wit_types::GameCandidate {
        wit_types::GameCandidate {
            reference: wit_types::ExternalReference {
                provider_id: provider.into(),
                external_id: external.into(),
            },
            title: title.into(),
            sort_title: None,
            platform: None,
            installed: false,
        }
    }

    fn page(
        games: Vec<wit_types::GameCandidate>,
        next_cursor: Option<&str>,
        complete: bool,
    ) -> wit_runner::RunnerGamePage {
        wit_runner::RunnerGamePage {
            games,
            page: wit_types::PageInfo {
                next_cursor: next_cursor.map(str::to_owned),
                complete,
            },
        }
    }

    #[test]
    fn a_page_is_rejected_when_it_breaks_the_hosts_rules() {
        let good = page(vec![candidate("prov", "one", "One")], None, true);
        assert!(validate_discovery_page(good, 4).is_ok());

        assert_eq!(
            validate_discovery_page(
                page(
                    vec![candidate("prov", "one", "One"), candidate("prov", "two", "Two")],
                    None,
                    true,
                ),
                1,
            )
            .unwrap_err(),
            PluginRuntimeError::InvalidResult("page longer than asked")
        );
        assert_eq!(
            validate_discovery_page(
                page(
                    vec![candidate("prov", "one", "One"), candidate("prov", "one", "Again")],
                    None,
                    true,
                ),
                4,
            )
            .unwrap_err(),
            PluginRuntimeError::InvalidResult("duplicate reference")
        );
        assert_eq!(
            validate_discovery_page(
                page(vec![candidate("prov", "one", "One")], Some("next"), true),
                4,
            )
            .unwrap_err(),
            PluginRuntimeError::InvalidResult("cursor after completion")
        );
        assert_eq!(
            validate_discovery_page(
                page(vec![candidate("prov", "one", "One")], Some("../escape"), false),
                4,
            )
            .unwrap_err(),
            PluginRuntimeError::InvalidResult("page cursor")
        );
        assert_eq!(
            validate_discovery_page(
                page(vec![candidate("prov", "../etc", "One")], None, true),
                4,
            )
            .unwrap_err(),
            PluginRuntimeError::InvalidResult("candidate reference")
        );
        assert_eq!(
            validate_discovery_page(
                page(vec![candidate("prov", "one", "One\u{7}Two")], None, true),
                4,
            )
            .unwrap_err(),
            PluginRuntimeError::InvalidResult("candidate title")
        );
    }

    #[test]
    fn an_entry_name_is_one_component_or_nothing() {
        assert!(valid_entry_name("alpha.rom"));
        assert!(!valid_entry_name(""));
        assert!(!valid_entry_name(".."));
        assert!(!valid_entry_name("../secret.txt"));
        assert!(!valid_entry_name("nested/alpha.rom"));
        assert!(!valid_entry_name("alpha\\beta"));
        assert!(!valid_entry_name("alpha\u{0}.rom"));
    }

    // -----------------------------------------------------------------------
    // Through the scheduler
    // -----------------------------------------------------------------------

    /// The scheduler and the host share one cancel token. This is the test that
    /// proves it: cancelling the *job* has to interrupt the component already
    /// running inside Wasmtime, not just stop the queue behind it.
    #[test]
    fn cancelling_a_scheduled_job_interrupts_the_running_component() {
        let library = FixtureLibrary::new("scheduled-cancel");
        let harness = Harness::new(
            PluginLimits {
                interactive_fuel: 1 << 42,
                interactive_deadline: Duration::from_secs(30),
                epoch_tick: Duration::from_millis(5),
                ..PluginLimits::default()
            },
            Some(&library),
        );
        let handle = harness
            .runtime
            .submit(
                &harness.prepared,
                FIXTURE_PLUGIN_ID,
                &harness.grants,
                PluginRequest::PrepareLaunch {
                    profile_id: FIXTURE_PROFILE.into(),
                    game_reference: "fixture:spin".into(),
                },
            )
            .unwrap();

        // 150 ms is the display budget, not the deadline: the job is still
        // running when it expires, which is exactly when the UI would stop
        // waiting and offer to cancel.
        let handle = handle
            .wait_for(INTERACTIVE_DISPLAY_BUDGET)
            .err()
            .expect("an endless component should outlive the display budget");
        handle.cancel();
        let started = Instant::now();
        assert_eq!(handle.wait().unwrap_err(), JobError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            !harness
                .runtime
                .scheduler()
                .health(FIXTURE_PLUGIN_ID)
                .degraded
        );
    }

    #[test]
    fn a_scheduled_invocation_returns_a_validated_result() {
        let library = FixtureLibrary::new("scheduled-ok");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let invocation = harness
            .runtime
            .submit(
                &harness.prepared,
                FIXTURE_PLUGIN_ID,
                &harness.grants,
                PluginRequest::HealthCheck,
            )
            .unwrap()
            .wait()
            .unwrap();
        assert!(matches!(
            invocation.response,
            PluginResponse::Health(PluginHealth { ready: true, .. })
        ));
    }

    /// Three rejected results in a row park the plugin, and the next submission
    /// is refused rather than queued.
    #[test]
    fn repeated_invalid_results_park_the_plugin() {
        let library = FixtureLibrary::new("scheduled-degraded");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        for _ in 0..DEFAULT_MAX_CONSECUTIVE_FAILURES {
            let error = harness
                .runtime
                .submit(
                    &harness.prepared,
                    FIXTURE_PLUGIN_ID,
                    &harness.grants,
                    PluginRequest::PrepareLaunch {
                        profile_id: FIXTURE_PROFILE.into(),
                        game_reference: "fixture:bad-mode".into(),
                    },
                )
                .unwrap()
                .wait()
                .unwrap_err();
            assert_eq!(
                error,
                JobError::Runtime(PluginRuntimeError::InvalidResult("intent mode"))
            );
        }
        assert!(
            harness
                .runtime
                .scheduler()
                .health(FIXTURE_PLUGIN_ID)
                .degraded
        );
        assert!(matches!(
            harness
                .runtime
                .submit(
                    &harness.prepared,
                    FIXTURE_PLUGIN_ID,
                    &harness.grants,
                    PluginRequest::HealthCheck,
                )
                .unwrap_err(),
            SubmitError::Degraded { .. }
        ));

        harness.runtime.scheduler().resume(FIXTURE_PLUGIN_ID);
        assert!(
            harness
                .runtime
                .submit(
                    &harness.prepared,
                    FIXTURE_PLUGIN_ID,
                    &harness.grants,
                    PluginRequest::HealthCheck,
                )
                .unwrap()
                .wait()
                .is_ok()
        );
    }

    // -----------------------------------------------------------------------
    // Cost
    // -----------------------------------------------------------------------

    /// The plan asks for numbers, so this prints them. Run with `--nocapture`
    /// to read the line; the assertions only guard the shape, because a timing
    /// threshold in CI is a flake waiting for a busy machine.
    ///
    /// The comparison that matters is *prepared against cold*: a prepared
    /// component skips compilation, which is three orders of magnitude more
    /// expensive than everything else a call does.
    #[test]
    fn reports_what_a_prepared_component_costs_against_a_cold_one() {
        let library = FixtureLibrary::new("cost");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let launch = PluginRequest::PrepareLaunch {
            profile_id: FIXTURE_PROFILE.into(),
            game_reference: "fixture:ok".into(),
        };
        let discover = PluginRequest::DiscoverPage {
            profile_id: FIXTURE_PROFILE.into(),
            cursor: None,
            limit: 10,
        };

        // Warm the engine's own caches first so the compile figure below is a
        // compile and not a first-touch of Cranelift.
        let _ = harness.call(launch.clone()).unwrap();

        let compile_started = Instant::now();
        let cold = harness
            .runtime
            .prepare_component(FIXTURE, FIXTURE_SHA256)
            .unwrap();
        let compile = compile_started.elapsed();

        let prepared_launch = harness.call_with(launch.clone(), &Arc::new(AtomicBool::new(false)))
            .unwrap();
        let prepared_discover = harness
            .call_with(discover, &Arc::new(AtomicBool::new(false)))
            .unwrap();
        let first_launch = harness
            .runtime
            .invoke(
                &cold,
                FIXTURE_PLUGIN_ID,
                &harness.grants,
                next_correlation_id(),
                &Arc::new(AtomicBool::new(false)),
                &launch,
            )
            .unwrap();

        println!(
            "fixture {} bytes\n  compile            {:?}\n  prepare-launch     instantiate {:?} call {:?} fuel {}\n  discover-page      instantiate {:?} call {:?} fuel {}\n  cold prepare-launch (compile + instantiate + call) {:?}",
            FIXTURE.len(),
            compile,
            prepared_launch.cost.instantiation,
            prepared_launch.cost.call,
            prepared_launch.cost.fuel_used,
            prepared_discover.cost.instantiation,
            prepared_discover.cost.call,
            prepared_discover.cost.fuel_used,
            compile
                .saturating_add(first_launch.cost.instantiation)
                .saturating_add(first_launch.cost.call),
        );
        assert!(prepared_launch.cost.fuel_used > 0);
        assert!(prepared_discover.cost.fuel_used > prepared_launch.cost.fuel_used);
        // Preparing ahead of the first call is the whole point: reusing a
        // compiled component must cost far less than compiling it again.
        assert!(compile > prepared_launch.cost.instantiation);
    }
}
