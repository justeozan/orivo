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

use crate::plugin_manifest::{
    CapabilityGrant, CapabilityScope, GrantValidationError, PluginCapability, PluginExtension,
    ValidatedPluginManifest, valid_opaque_id,
};
use crate::plugin_scheduler::{JobError, JobHandle, PluginScheduler, SchedulerLimits, SubmitError};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Write},
    path::{Component as PathComponent, Path, PathBuf},
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

/// Room for host frames on top of the wasm stack, on every thread that runs
/// guest code.
///
/// `max_wasm_stack` is not a limit on its own. Wasmtime sets the trap threshold
/// to `stack_pointer - max_wasm_stack` and does not clamp it to the thread it is
/// running on (`wasmtime/src/runtime/func.rs`), and it does not count host frames
/// at all. A thread smaller than the sum therefore overflows *below* wasmtime's
/// check — and an overflow taken in host code, whether in one of our host
/// functions or in a trampoline copying a result out, is an abort rather than a
/// trap. A thread stack is lazily-committed address space, so being far past the
/// deepest frame the host can reach costs nothing.
pub const PLUGIN_HOST_STACK_HEADROOM_BYTES: usize = 6 * 1024 * 1024;

/// The stack every thread that enters Wasmtime must have. Guest code runs on
/// scheduler workers and nowhere else, which is what makes this enforceable.
pub const PLUGIN_THREAD_STACK_BYTES: usize =
    PLUGIN_WASM_STACK_BYTES + PLUGIN_HOST_STACK_HEADROOM_BYTES;

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
/// How many entries the host will look at before it stops. Larger than the
/// listing it returns so the returned page is the first 256 *by name* rather
/// than whichever 256 the filesystem happened to hand over first.
const MAX_DIRECTORY_SCAN: usize = 4096;
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

/// Two bounded rings, and the split matters: a plugin can call `host-journal`
/// as often as it likes, and sharing one ring would let it evict the host's
/// record of the refusals it just earned. Decisions are the host's; messages are
/// the plugin's, and only the plugin's own ring can be flooded.
///
/// Both are deliberately in memory and capped: the journal exists to explain the
/// last failure to a user and to let a test assert that a refusal was recorded,
/// not to become a log file a plugin can grow.
#[derive(Debug, Default)]
pub struct PluginJournal {
    decisions: Mutex<VecDeque<JournalEntry>>,
    messages: Mutex<VecDeque<JournalEntry>>,
}

impl PluginJournal {
    pub fn record(
        &self,
        correlation_id: CorrelationId,
        plugin_id: &str,
        decision: &'static str,
        detail: impl Into<String>,
    ) {
        self.push(&self.decisions, correlation_id, plugin_id, decision, detail);
    }

    /// Text a plugin chose. Kept apart from the host's decisions so a chatty
    /// component cannot push them out of the ring.
    fn record_plugin_message(
        &self,
        correlation_id: CorrelationId,
        plugin_id: &str,
        detail: impl Into<String>,
    ) {
        self.push(
            &self.messages,
            correlation_id,
            plugin_id,
            "plugin-log",
            detail,
        );
    }

    fn push(
        &self,
        ring: &Mutex<VecDeque<JournalEntry>>,
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
        // `eprintln!` panics when stderr is gone, and this is called from inside
        // the scheduler. A journal line is not worth poisoning a lock over.
        let _ = writeln!(
            std::io::stderr(),
            "orivo plugin [{}] {} {}: {}",
            entry.correlation_id,
            entry.plugin_id,
            entry.decision,
            entry.detail
        );
        if let Ok(mut ring) = ring.lock() {
            if ring.len() == MAX_JOURNAL_ENTRIES {
                ring.pop_front();
            }
            ring.push_back(entry);
        }
    }

    #[allow(dead_code)]
    pub fn entries(&self) -> Vec<JournalEntry> {
        Self::snapshot(&self.decisions)
    }

    #[allow(dead_code)]
    pub fn plugin_messages(&self) -> Vec<JournalEntry> {
        Self::snapshot(&self.messages)
    }

    fn snapshot(ring: &Mutex<VecDeque<JournalEntry>>) -> Vec<JournalEntry> {
        ring.lock()
            .map(|ring| ring.iter().cloned().collect())
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
    directories: BTreeMap<String, Arc<GrantedDirectory>>,
}

/// One approved folder, held open for as long as the grant lives.
///
/// The descriptor is the grant. `O_NOFOLLOW` judges the last component of a path
/// and nothing above it, so a *parent* of the granted folder replaced by a
/// symbolic link — which needs write access to that parent, not to the folder
/// itself — silently redirects every later read. A path is re-resolved on every
/// use and can therefore be answered differently each time; a descriptor names
/// the directory the user actually approved, once, and `openat` reads relative to
/// it. A swap afterwards changes nothing.
pub struct GrantedDirectory {
    path: PathBuf,
    #[cfg(unix)]
    handle: File,
}

impl std::fmt::Debug for GrantedDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrantedDirectory")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Two grants name the same folder when they resolved to the same path. The
/// descriptor is an implementation detail of reaching it, not part of its
/// identity, and comparing raw file descriptors would make equality depend on
/// the order in which grants happened to be opened.
impl PartialEq for GrantedDirectory {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Eq for GrantedDirectory {}

impl GrantedDirectory {
    fn open(path: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Links *on the way to* the grant are followed here, deliberately:
            // this is the moment the user pointed at a folder, and on macOS the
            // ordinary temporary and home directories live behind one. What must
            // not be re-resolved is everything after it.
            let handle = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
                .open(path)?;
            Ok(Self {
                path: path.to_path_buf(),
                handle,
            })
        }
        #[cfg(not(unix))]
        {
            // Windows has no `openat`, so the grant is still a path here and the
            // swap above is still reachable. `FILE_FLAG_OPEN_REPARSE_POINT` keeps
            // the *entry* honest, which is the half that can be kept.
            if !path.is_dir() {
                return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
            }
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
    }

    /// Opens one entry of this directory, relative to the handle.
    ///
    /// `O_NOFOLLOW` refuses a symbolic link instead of resolving it, and
    /// `O_NONBLOCK` means a FIFO does not park this worker where neither the
    /// epoch nor a cancellation can reach it. The caller still has to ask the
    /// descriptor what it opened: a directory and a character device both open
    /// happily here.
    fn open_entry(&self, name: &str) -> std::io::Result<File> {
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};

            let name = CString::new(name)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            // Safety: `handle` is an open directory descriptor borrowed for the
            // length of the call, and `name` is NUL-terminated.
            let descriptor = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Safety: `openat` just returned this descriptor and nothing else
            // owns it.
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
        #[cfg(not(unix))]
        {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT)
                .open(self.path.join(name))
        }
    }
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
                    // Opened here rather than at the call, because this is the
                    // moment the grant is made. A folder the host cannot open as
                    // a directory now is not a scope it can honour later.
                    let directory = GrantedDirectory::open(path)
                        .map_err(|_| GrantValidationError::InvalidScope(grant.capability))?;
                    resolved.directories.insert(id.clone(), Arc::new(directory));
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

    fn directory(&self, id: &str) -> Option<&Arc<GrantedDirectory>> {
        self.directories.get(id)
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

/// Which ceiling a store ran into. The difference is whose fault it is: a
/// component that asked for more than one instance may have is misbehaving, but
/// one refused because *other* plugins had already filled the global budget is
/// an innocent bystander and must not be marked degraded for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryLimitKind {
    Instance,
    Host,
}

/// The `ResourceLimiter` for one store. It refuses growth rather than trapping
/// so a well-written guest can fail its own allocation gracefully; `hit_limit`
/// is what lets the host report the resulting abort as a memory limit instead
/// of an anonymous trap.
#[derive(Debug)]
struct StoreMemoryGuard {
    limits: PluginLimits,
    budget: Arc<MemoryBudget>,
    /// Bytes this store currently holds against the global budget. It only ever
    /// grows; the store's `Drop` is what returns it.
    charged: usize,
    hit_limit: Option<MemoryLimitKind>,
}

impl ResourceLimiter for StoreMemoryGuard {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let extra = desired.saturating_sub(current);
        // Wasmtime asks the limiter before it checks the memory's own declared
        // maximum, and refuses the growth afterwards regardless of the answer.
        // Refusing it here is what keeps a charge from being taken for a growth
        // that was never going to happen — and it is why this guard has no
        // refund. There is no "growth succeeded" callback, so a refund could not
        // tell whether it was giving back the growth that just failed or the one
        // that succeeded before it; Wasmtime also reports a failure it never
        // asked about at all when a size is unrepresentable, which turns any such
        // refund into a budget a component can mint on demand.
        if maximum.is_some_and(|maximum| desired > maximum) {
            self.hit_limit = Some(MemoryLimitKind::Instance);
            return Ok(false);
        }
        // The ceiling is the store's whole footprint, not one memory's. A
        // component may define several — `memories_per_store` allows four, for
        // the core module instances inside one component — and four memories of
        // the per-instance size are not a per-instance limit.
        if self.charged.saturating_add(extra) > self.limits.instance_memory_bytes {
            self.hit_limit = Some(MemoryLimitKind::Instance);
            return Ok(false);
        }
        if !self.budget.charge(extra, self.limits.total_memory_bytes) {
            self.hit_limit = Some(MemoryLimitKind::Host);
            return Ok(false);
        }
        self.charged = self.charged.saturating_add(extra);
        Ok(true)
    }

    /// Deliberately left as the default, which only logs: a growth this guard
    /// allowed and the operating system then refused stays charged until the
    /// store is dropped. Over-counting for the length of one invocation is the
    /// safe direction to be wrong in, and the alternative — giving budget back
    /// here — is forgeable, as the comment on `memory_growing` explains.

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.limits.table_elements {
            self.hit_limit = Some(MemoryLimitKind::Instance);
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
    /// The wall-clock end of the call. Ticks alone undercount: `Continue(1)`
    /// re-arms the deadline relative to the *current* epoch, so every tick that
    /// passed while the guest sat inside a host call collapses into one.
    deadline_at: Instant,
    interruption: Option<Interruption>,
    host_calls: u32,
    /// Whether the exhausted host-call budget has already been journalled. Every
    /// further call is refused in silence: a component that keeps asking would
    /// otherwise push the host's own decisions out of the ring by being refused.
    host_call_budget_reported: bool,
    bytes_read: u64,
}

impl HostState {
    fn spend_host_call(&mut self) -> Result<(), wit_types::PluginError> {
        if self.host_calls >= MAX_HOST_CALLS_PER_INVOCATION {
            if !self.host_call_budget_reported {
                self.host_call_budget_reported = true;
                self.journal.record(
                    self.correlation_id,
                    &self.plugin_id,
                    "host-call-budget",
                    "refused: the invocation exhausted its host-call budget",
                );
            }
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
    fn granted_directory(
        &self,
        grant: &str,
    ) -> Result<Arc<GrantedDirectory>, wit_types::PluginError> {
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
            Some(directory) => Ok(Arc::clone(directory)),
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

fn plugin_error(code: wit_types::PluginErrorCode, message: &str) -> wit_types::PluginError {
    wit_types::PluginError {
        code,
        message: message.to_owned(),
        retryable: false,
    }
}

impl host_journal::Host for HostState {
    fn log(&mut self, level: host_journal::JournalLevel, message: String) {
        // Metered like any other host call. It has no error channel, so an
        // exhausted budget drops the line — but it must not be the one host
        // import a component can call without limit, because each call costs the
        // host a string copy out of guest memory.
        if self.spend_host_call().is_err() {
            return;
        }
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
        // Bound the *input*, not just the output. Filtering the whole string
        // first would make a 64 MiB message of control bytes cost a full scan to
        // produce 512 bytes, and that scan is time the deadline pays for.
        let mut scanned = 0;
        for character in message.chars() {
            scanned += character.len_utf8();
            if scanned > MAX_JOURNAL_MESSAGE_BYTES
                || detail.len() + character.len_utf8() > MAX_JOURNAL_MESSAGE_BYTES
            {
                break;
            }
            if !character.is_control() {
                detail.push(character);
            }
        }
        self.journal
            .record_plugin_message(self.correlation_id, &self.plugin_id, detail);
    }
}

impl host_files::Host for HostState {
    fn list_directory(
        &mut self,
        grant: String,
    ) -> Result<Vec<host_files::DirectoryEntry>, wit_types::PluginError> {
        self.spend_host_call()?;
        let directory = self.granted_directory(&grant)?;
        let Ok(entries) = fs::read_dir(&directory.path) else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::Unavailable,
                "That folder is no longer readable.",
            ));
        };
        let mut listing = Vec::new();
        // Two bounds, because they answer different questions: how much of a
        // directory the host is willing to walk, and how much of it a plugin is
        // allowed to hear about. Sorting before the second one is what makes the
        // answer the same on every run — `read_dir` order is not.
        for entry in entries.filter_map(Result::ok).take(MAX_DIRECTORY_SCAN) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() > MAX_ENTRY_NAME_BYTES || !valid_entry_name(&name) {
                continue;
            }
            // `read_dir` walks a path, and a path is what a swapped parent
            // redirects. The name is therefore only a suggestion: every fact
            // reported below is asked of a descriptor opened relative to the
            // granted directory's own handle, so an entry the approved folder
            // does not have cannot be listed at all, and a symbolic link is
            // refused by the open rather than described. The worst a swap can
            // still do is *hide* entries, which is not a way out of the grant.
            let Ok(opened) = directory.open_entry(&name) else {
                continue;
            };
            let Ok(metadata) = opened.metadata() else {
                continue;
            };
            listing.push(host_files::DirectoryEntry {
                name,
                byte_size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                directory: metadata.is_dir(),
            });
        }
        listing.sort_by(|left, right| left.name.cmp(&right.name));
        listing.truncate(MAX_DIRECTORY_ENTRIES);
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
        let directory = self.granted_directory(&grant)?;
        if name.len() > MAX_ENTRY_NAME_BYTES || !valid_entry_name(&name) {
            return Err(plugin_error(
                wit_types::PluginErrorCode::InvalidInput,
                "That is not a name inside the allowed folder.",
            ));
        }
        // Open through the grant's own handle, then judge the descriptor.
        // Checking a path and reading it again are two different objects if
        // anything can write to the granted folder in between: a symbolic link
        // reads outside the grant, a FIFO blocks this worker forever — neither
        // the epoch nor a cancellation can reach a thread parked in `read` — and
        // a character device has no size to bound. The open refuses the first
        // two, and the kind, size and ownership below are asked of the
        // descriptor rather than of the name.
        let Ok(file) = directory.open_entry(&name) else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::Unavailable,
                "That file is no longer available.",
            ));
        };
        let Ok(metadata) = file.metadata() else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::Unavailable,
                "That file is no longer available.",
            ));
        };
        if let Some(refusal) = refuse_entry(&EntryFacts::of(&metadata), host_account()) {
            return Err(plugin_error(
                wit_types::PluginErrorCode::PermissionDenied,
                refusal.message(),
            ));
        }
        if self.bytes_read.saturating_add(metadata.len()) > MAX_HOST_READ_BYTES {
            return Err(plugin_error(
                wit_types::PluginErrorCode::RateLimited,
                "This plugin read too much in one call.",
            ));
        }
        // The descriptor's reported length is a hint, not a contract: a file that
        // grows between `metadata` and the read must not become an unbounded
        // allocation, so the reader carries the ceiling itself.
        let ceiling = MAX_HOST_FILE_BYTES.min(MAX_HOST_READ_BYTES.saturating_sub(self.bytes_read));
        let Some(bytes) = read_at_most(file, ceiling) else {
            return Err(plugin_error(
                wit_types::PluginErrorCode::PermissionDenied,
                "That entry is not a readable file of an allowed size.",
            ));
        };
        self.bytes_read = self.bytes_read.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }
}

/// Turns an unwind into a typed refusal. Generic over the work so the catching
/// itself is testable: the production caller hands it a compilation, and a test
/// hands it a panic.
fn without_unwinding<T>(
    work: impl FnOnce() -> Result<T, PluginRuntimeError>,
) -> Result<T, PluginRuntimeError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(outcome) => outcome,
        Err(_) => Err(PluginRuntimeError::InvalidComponent),
    }
}

/// Reads at most `ceiling` bytes, and refuses rather than quietly truncating.
///
/// Split out from `read_file` because the bound is otherwise only reachable by
/// racing a file that grows between its metadata and its read — and a ceiling
/// whose test cannot fail is not a ceiling. `take(ceiling + 1)` is what makes the
/// overshoot visible.
fn read_at_most(reader: impl Read, ceiling: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(ceiling.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= ceiling).then_some(bytes)
}

/// Reserved device names. On Windows these do not name files at all: `CON` is
/// the console — and blocks a worker exactly as a FIFO does — and `COM1` is a
/// serial port. Win32 also ignores everything from the first dot, so `NUL.rom` is
/// still `NUL`. They are refused on every platform because this validator is a
/// pure function and Orivo's CI runs `cargo test` on macOS, not on Windows.
/// Superscript digits Win32 folds onto their ASCII counterparts, so `COM¹` is
/// `COM1`. An ASCII-only comparison does not see them.
const WINDOWS_SUPERSCRIPT_DIGITS: [(char, char); 3] =
    [('\u{b9}', '1'), ('\u{b2}', '2'), ('\u{b3}', '3')];

const WINDOWS_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
];

fn is_windows_device_name(resolved: &str) -> bool {
    // The stem gets the same treatment as the whole name: Win32 trims the spaces
    // before the dot too, so `NUL .rom` is `NUL`.
    let stem = resolved
        .split('.')
        .next()
        .unwrap_or(resolved)
        .trim_end_matches(' ');
    let folded = stem
        .chars()
        .map(|character| {
            WINDOWS_SUPERSCRIPT_DIGITS
                .iter()
                .find_map(|(superscript, digit)| (*superscript == character).then_some(*digit))
                .unwrap_or(character)
        })
        .collect::<String>();
    WINDOWS_DEVICE_NAMES
        .iter()
        .any(|device| folded.eq_ignore_ascii_case(device))
}

/// What the host knows about an entry once it has opened it, as plain values.
///
/// Asking the descriptor rather than the name is what makes these trustworthy;
/// keeping them as data is what makes the rule below a pure function, which is
/// the only way one of its cases can be tested at all — reproducing that one for
/// real needs a second local account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EntryFacts {
    file: bool,
    byte_size: u64,
    /// How many names this file answers to. More than one is a hard link.
    links: u64,
    owner: u32,
}

impl EntryFacts {
    fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                file: metadata.is_file(),
                byte_size: metadata.len(),
                links: metadata.nlink(),
                owner: metadata.uid(),
            }
        }
        #[cfg(not(unix))]
        {
            // Windows reports a link count only through a separate query and has
            // no uid to compare, so the ownership rule below never fires there.
            // Said out loud rather than silently approximated.
            Self {
                file: metadata.is_file(),
                byte_size: metadata.len(),
                links: 1,
                owner: 0,
            }
        }
    }
}

/// Why the host will not hand an entry's bytes to a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryRefusal {
    NotAFile,
    TooLarge,
    /// A file with more than one name, owned by another local account.
    ForeignHardLink,
}

impl EntryRefusal {
    fn message(self) -> &'static str {
        match self {
            Self::NotAFile | Self::TooLarge => {
                "That entry is not a readable file of an allowed size."
            }
            Self::ForeignHardLink => {
                "That entry is another account's file, under a second name in your folder."
            }
        }
    }
}

fn refuse_entry(facts: &EntryFacts, host_owner: u32) -> Option<EntryRefusal> {
    if !facts.file {
        return Some(EntryRefusal::NotAFile);
    }
    if facts.byte_size > MAX_HOST_FILE_BYTES {
        return Some(EntryRefusal::TooLarge);
    }
    // One name for another account's file is a folder the user pointed at on
    // purpose — a shared library, something under `/Applications`. A *second*
    // name for it is not: linking a file does not require being able to read it,
    // so this is the shape a plugin is being used to read something on someone
    // else's behalf. The user's own hard links stay readable, because a
    // deduplicated library is an ordinary thing to have.
    if facts.links > 1 && facts.owner != host_owner {
        return Some(EntryRefusal::ForeignHardLink);
    }
    None
}

/// The account Orivo is running as. A grant authorises reading the user's own
/// files; it is not a way to read another account's.
fn host_account() -> u32 {
    #[cfg(unix)]
    {
        // Safety: `geteuid` reads this process's own identity and cannot fail.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Exactly one ordinary path component, and nothing that could leave the
/// granted directory on any platform this ships to.
///
/// The colon is the one that is easy to miss. `Path::join` replaces the whole
/// path when what it is given carries a prefix, so on Windows a grant rooted at
/// `C:\games` joined with `D:secrets.txt` becomes `D:secrets.txt` — the grant is
/// gone — and `C:x` resolves against that drive's current directory. Rejecting
/// `:` outright is what makes this checkable on a Unix CI, where `:` is an
/// ordinary character and `components()` would happily call it `Normal`.
fn valid_entry_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_ENTRY_NAME_BYTES
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
        || name.chars().any(char::is_control)
    {
        return false;
    }
    // Win32 strips trailing spaces and dots before it resolves a name, so this is
    // the form Windows would actually look up. An empty one names the directory
    // itself.
    let resolved = name.trim_end_matches([' ', '.']);
    if resolved.is_empty() || is_windows_device_name(resolved) {
        return false;
    }
    let mut components = Path::new(name).components();
    let single = matches!(components.next(), Some(PathComponent::Normal(first)) if first == OsStr::new(name));
    single && components.next().is_none()
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
    /// Refused because Orivo's global plugin memory ceiling was already full.
    /// The plugin asked for something reasonable at a bad moment.
    HostMemoryExhausted,
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
                | Self::HostMemoryExhausted
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
                write!(
                    formatter,
                    "The plugin asked for more memory than it may use."
                )
            }
            Self::HostMemoryExhausted => write!(
                formatter,
                "Orivo's plugins are already using all the memory it sets aside for them."
            ),
            Self::Trapped => write!(formatter, "The plugin stopped unexpectedly."),
            Self::Plugin { message, .. } => write!(formatter, "{message}"),
            Self::InvalidResult(reason) => {
                write!(
                    formatter,
                    "The plugin returned an unusable result ({reason})."
                )
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
            // A 64-bit memory is the one way a guest reaches Wasmtime's "growth
            // exceeds address space" path, which is the only place it tells a
            // `ResourceLimiter` that a growth failed *without having asked it
            // first*. The canonical ABI has no use for one.
            //
            // Multi-memory stays on, and must: Wasmtime synthesises its own
            // adapter modules for values crossing between composed components,
            // those adapters import both memories, and it validates them with
            // the engine's features behind an `expect`. Turning it off does not
            // refuse such a component — it panics.
            .wasm_memory64(false)
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
    /// Bytes currently held against the global ceiling. A leak here is
    /// invisible from the outside until the ceiling stops working, so a test
    /// reads it directly.
    #[cfg(test)]
    fn committed_memory(&self) -> usize {
        self.inner.budget.committed.load(Ordering::Acquire)
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
            Ok(Err(JobError::Panicked)) => Err(PluginRuntimeError::Trapped),
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
        self.compile(bytes).map(|_| ())
    }

    /// Compiles once and keeps the result. Preparing ahead of a first call is
    /// the difference the plan asks to measure; `invoke` accepts the prepared
    /// component so the cost is paid where it is visible.
    pub fn prepare_component(
        &self,
        bytes: &[u8],
        sha256: &str,
    ) -> Result<PreparedComponent, PluginRuntimeError> {
        self.compile(bytes).map(|component| PreparedComponent {
            component,
            sha256: sha256.to_owned(),
        })
    }

    /// Compiling untrusted bytes must produce a component or a typed error, and
    /// never an unwind.
    ///
    /// Wasmtime's component translator asserts on its own invariants — it
    /// `expect`s that the adapter modules it generates validate, for one — and
    /// the caller here is `install_plugin_from_file`, which runs on the main
    /// thread. A panic there is an aborted app rather than a refused package, so
    /// the guard is worth having even though every panic behind it is a bug
    /// somewhere: a bug that refuses a plugin is a support question, and a bug
    /// that closes Orivo is an outage. Nothing is installed into the engine until
    /// compilation returns, so there is no half-registered module to inherit.
    fn compile(&self, bytes: &[u8]) -> Result<Component, PluginRuntimeError> {
        without_unwinding(|| {
            Component::new(&self.inner.engine, bytes)
                .map_err(|_| PluginRuntimeError::InvalidComponent)
        })
    }

    /// One call into a component, under grants and limits, from start to
    /// validated result.
    ///
    /// The store is built, used and dropped here. A component therefore keeps
    /// nothing between calls — no cached handle, no open directory, no linear
    /// memory — which is what makes a per-invocation fuel and memory budget
    /// meaningful instead of cumulative.
    ///
    /// Private, and it has to stay private: this runs guest code, and guest code
    /// may only run on a thread with [`PLUGIN_THREAD_STACK_BYTES`] of stack.
    /// `submit` is the door, because the scheduler is what guarantees that.
    fn invoke(
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
                    hit_limit: None,
                },
                cancel: Arc::clone(cancel),
                ticks_remaining: ticks,
                deadline_at: Instant::now() + deadline,
                interruption: None,
                host_calls: 0,
                host_call_budget_reported: false,
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
            // Whichever of the two runs out first. The tick count is the
            // deterministic one — a test can drive it by hand — and the clock is
            // the honest one, because time spent in a host call advances it and
            // does not advance the count.
            if state.ticks_remaining == 0 || Instant::now() >= state.deadline_at {
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
                validate_launch_intent(intent, &store.data().plugin_id, profile_id, game_reference)
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
        match state.memory.hit_limit {
            Some(MemoryLimitKind::Instance) => return PluginRuntimeError::MemoryLimit,
            // Refused because every other live instance had already filled the
            // ceiling. Not this plugin's doing, so it must not be its failure.
            Some(MemoryLimitKind::Host) => return PluginRuntimeError::HostMemoryExhausted,
            None => {}
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

fn validate_identity(identity: wit_core::Identity) -> Result<PluginIdentity, PluginRuntimeError> {
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

fn validate_health(health: wit_core::Health) -> Result<PluginHealth, PluginRuntimeError> {
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
    plugin_id: &str,
    profile_id: &str,
    game_reference: &str,
) -> Result<PluginLaunchIntent, PluginRuntimeError> {
    if intent.profile_id != profile_id {
        return Err(PluginRuntimeError::InvalidResult("intent profile"));
    }
    if intent.game_reference != game_reference {
        return Err(PluginRuntimeError::InvalidResult("intent game reference"));
    }
    // A plugin prepares launches for itself and for nothing else. Leaving this
    // to the opaque-id grammar alone would let one installed runner hand back
    // another's id, and whatever resolves a runner id later would believe it.
    if intent.runner_id != plugin_id || !valid_opaque_id(&intent.runner_id, MAX_RESULT_ID_BYTES) {
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
    use crate::plugin_manifest::{ArtifactDescriptor, ArtifactKind, PLUGIN_SDK_V1, PluginManifest};
    use crate::plugin_scheduler::JobError;
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
    const FIXTURE_SHA256: &str = "36ba4a71ad5a7973dd7e54cd702eb1926d5c3fa2f1268cf25b2cc8a4802d7dde";
    /// A component whose only import is WASI. Also built by `build.sh`, from
    /// hand-written component text rather than a second Rust guest.
    const WASI_IMPORT: &[u8] = include_bytes!("../fixtures/wasi-import.wasm");
    const WASI_IMPORT_SHA256: &str =
        "4b7909ec90668f639b6023c4b844a8bf08201fda44125b68acc44f24b7b12630";
    /// A component whose core module declares a 64-bit linear memory. Also built
    /// by `build.sh`, from hand-written component text.
    const MEMORY64: &[u8] = include_bytes!("../fixtures/memory64.wasm");
    const MEMORY64_SHA256: &str =
        "864557361c1ee165e36a852e29c7ad04ae387811096e63adb349bea7d213b614";
    /// Two composed components, each with its own memory, and a string crossing
    /// between them — the shape that makes Wasmtime synthesise an adapter module
    /// importing both memories.
    const COMPOSED_MEMORIES: &[u8] = include_bytes!("../fixtures/composed-memories.wasm");
    const COMPOSED_MEMORIES_SHA256: &str =
        "db7ab4e72cadfabed845032b85baf0f9ff884e8cca50fa8ef18e305846b40d82";

    const FIXTURE_PLUGIN_ID: &str = "com.orivo.fixture-runner";
    const FIXTURE_PROFILE: &str = "fixture-profile-1";
    const GAMES_GRANT: &str = "fixture-games";

    /// Guest code only ever runs on a thread with the host's stack. The tests
    /// hold themselves to the scheduler's rule rather than trusting whatever
    /// stack libtest happened to give them.
    fn on_a_host_sized_thread<T: Send>(work: impl FnOnce() -> T + Send) -> T {
        thread::scope(|scope| {
            thread::Builder::new()
                .stack_size(PLUGIN_THREAD_STACK_BYTES)
                .spawn_scoped(scope, work)
                .expect("a worker-sized thread")
                .join()
                .expect("the invocation did not unwind")
        })
    }

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
            match library {
                Some(library) => {
                    Self::with_directories(limits, &[GAMES_GRANT], &library.directories())
                }
                None => {
                    let runtime = PluginRuntime::with_limits(limits, EpochMode::Threaded).unwrap();
                    let prepared = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
                    Self {
                        runtime,
                        prepared,
                        grants: PluginGrants::none(),
                    }
                }
            }
        }

        /// The grants a test wants, rather than the fixture library's own. The
        /// ids and the map are separate arguments on purpose: a grant naming
        /// folders the map does not have is exactly the narrowed scope several
        /// tests below need.
        fn with_directories(
            limits: PluginLimits,
            ids: &[&str],
            directories: &BTreeMap<String, PathBuf>,
        ) -> Self {
            let runtime = PluginRuntime::with_limits(limits, EpochMode::Threaded).unwrap();
            let prepared = runtime.prepare_component(FIXTURE, FIXTURE_SHA256).unwrap();
            let grants = PluginGrants::resolve(
                &fixture_manifest(vec![
                    PluginCapability::RunnerPrepare,
                    PluginCapability::FilesRead,
                ]),
                &[files_grant(ids)],
                directories,
            )
            .unwrap();
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
            on_a_host_sized_thread(|| {
                self.runtime.invoke(
                    &self.prepared,
                    FIXTURE_PLUGIN_ID,
                    &self.grants,
                    next_correlation_id(),
                    cancel,
                    &request,
                )
            })
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

    /// Four production paths resolve a plugin, and they only share a compiled
    /// component cache, a memory ceiling, a journal and a failure counter if they
    /// share the runtime. A fresh `Engine` per path is how a plugin parked as
    /// degraded on one surface answers again on the next.
    #[test]
    fn the_shared_runtime_is_one_runtime() {
        let first = PluginRuntime::shared().unwrap();
        let second = PluginRuntime::shared().unwrap();
        assert!(Arc::ptr_eq(first.journal(), second.journal()));
        assert!(Arc::ptr_eq(first.scheduler(), second.scheduler()));
    }

    #[test]
    fn a_component_without_the_runner_world_is_not_invoked() {
        let runtime = PluginRuntime::new().unwrap();
        let prepared = runtime
            .prepare_component(EMPTY_COMPONENT, "0".repeat(64).as_str())
            .unwrap();
        let error = on_a_host_sized_thread(|| {
            runtime.invoke(
                &prepared,
                FIXTURE_PLUGIN_ID,
                &PluginGrants::none(),
                next_correlation_id(),
                &Arc::new(AtomicBool::new(false)),
                &PluginRequest::Identity,
            )
        })
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
            on_a_host_sized_thread(|| {
                runtime.invoke(
                    &prepared,
                    FIXTURE_PLUGIN_ID,
                    &PluginGrants::none(),
                    next_correlation_id(),
                    &Arc::new(AtomicBool::new(false)),
                    &PluginRequest::Identity,
                )
            })
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

        let PluginResponse::Identity(identity) = on_a_host_sized_thread(|| {
            runtime.invoke(
                &prepared,
                FIXTURE_PLUGIN_ID,
                &grants,
                next_correlation_id(),
                &cancel,
                &PluginRequest::Identity,
            )
        })
        .unwrap()
        .response
        else {
            panic!("expected an identity");
        };
        assert_eq!(identity.id, FIXTURE_PLUGIN_ID);

        let error = on_a_host_sized_thread(|| {
            runtime.invoke(
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
        })
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

        // Every shape of escape the grammar has to refuse, through the component
        // rather than through `valid_entry_name` directly: a name is only safe if
        // it is still refused after crossing the ABI.
        for escape in [
            "fixture:read-../secret",
            "fixture:read-/etc/passwd",
            "fixture:read-D:secret",
            "fixture:read-..\\..\\secret",
        ] {
            let error = harness.prepare(escape).unwrap_err();
            assert!(
                matches!(
                    error,
                    PluginRuntimeError::Plugin {
                        code: PluginErrorCode::InvalidInput,
                        ..
                    }
                ),
                "{escape} was answered with {error:?}"
            );
        }
    }

    /// The listing skips symbolic links, but a plugin can name an entry the
    /// listing never offered. Checking the path and then reading it again are two
    /// different objects, so the read has to refuse the link itself.
    #[cfg(unix)]
    #[test]
    fn reading_a_symlink_by_name_never_leaves_the_grant() {
        let library = FixtureLibrary::new("read-symlink");
        std::os::unix::fs::symlink(
            library.root.join("secret.txt"),
            library.games.join("link.rom"),
        )
        .unwrap();
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let error = harness.prepare("fixture:read-link").unwrap_err();
        assert!(
            matches!(
                error,
                PluginRuntimeError::Plugin {
                    code: PluginErrorCode::Unavailable,
                    ..
                }
            ),
            "reading a symlink out of the grant returned {error:?}"
        );
    }

    /// `O_NOFOLLOW` judges the last component of a path and nothing above it, so
    /// a grant is only as trustworthy as every directory on the way to it. This
    /// plants the swap the flag cannot see: the granted folder's *parent* is
    /// replaced by a symbolic link to a folder the user never approved, which
    /// needs write access to that parent rather than to the grant.
    ///
    /// Before the handle, `read_file` joined the grant's path and followed the
    /// link, so `fixture:read-secret` did not fail — it succeeded, reading a file
    /// from the attacker's folder.
    #[cfg(unix)]
    #[test]
    fn a_swapped_parent_cannot_redirect_a_granted_folder() {
        let root = temporary_root("swapped-parent");
        let library = root.join("library");
        let decoy = root.join("decoy");
        fs::create_dir_all(library.join("games")).unwrap();
        fs::create_dir_all(decoy.join("games")).unwrap();
        fs::write(library.join("games/alpha.rom"), b"Alpha Quest\n").unwrap();
        fs::write(decoy.join("games/secret.rom"), b"a keychain token").unwrap();

        let harness = Harness::with_directories(
            PluginLimits::default(),
            &[GAMES_GRANT],
            &BTreeMap::from([(GAMES_GRANT.to_string(), library.join("games"))]),
        );

        // The swap happens after the user granted the folder, which is the whole
        // point: the handle names the directory they approved, not the path.
        fs::rename(&library, root.join("library-real")).unwrap();
        std::os::unix::fs::symlink(&decoy, &library).unwrap();

        let error = harness.prepare("fixture:read-secret").unwrap_err();
        assert!(
            matches!(
                error,
                PluginRuntimeError::Plugin {
                    code: PluginErrorCode::Unavailable,
                    ..
                }
            ),
            "a swapped parent redirected the grant: {error:?}"
        );
        // And the folder the user really granted is still readable, so this is a
        // handle rather than a refusal of everything.
        assert!(harness.prepare("fixture:read-alpha").is_ok());

        let _ = fs::remove_dir_all(&root);
    }

    /// A FIFO is the one substitution that does not merely read the wrong file:
    /// a worker parked in `read` is a worker neither the epoch deadline nor a
    /// cancellation can reach, and there are only two of them.
    #[cfg(unix)]
    #[test]
    fn reading_a_fifo_by_name_does_not_park_a_worker() {
        let library = FixtureLibrary::new("read-fifo");
        let path = library.games.join("pipe.rom");
        let raw = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // Safety: a path this process owns, in a directory it just created.
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o644) }, 0);

        let harness = Harness::new(PluginLimits::default(), Some(&library));
        let handle = harness
            .runtime
            .submit(
                &harness.prepared,
                FIXTURE_PLUGIN_ID,
                &harness.grants,
                PluginRequest::PrepareLaunch {
                    profile_id: FIXTURE_PROFILE.into(),
                    game_reference: "fixture:read-pipe".into(),
                },
            )
            .unwrap();
        match handle.wait_for(Duration::from_secs(10)) {
            Ok(outcome) => {
                let JobError::Runtime(error) = outcome.unwrap_err() else {
                    panic!("expected the host's refusal");
                };
                assert!(
                    matches!(
                        error,
                        PluginRuntimeError::Plugin {
                            code: PluginErrorCode::PermissionDenied,
                            ..
                        }
                    ),
                    "reading a FIFO returned {error:?}"
                );
            }
            Err(handle) => {
                handle.cancel();
                panic!("a FIFO parked the worker");
            }
        }
    }

    /// The size check uses what the descriptor reports; this covers the bound
    /// that has to hold when that number is wrong. Removing `take(ceiling + 1)`
    /// makes it fail, which the file-size test alone does not.
    #[test]
    fn a_reader_that_outruns_its_ceiling_is_refused_rather_than_truncated() {
        use std::io::Cursor;

        assert_eq!(
            read_at_most(Cursor::new(vec![7u8; 100]), 100),
            Some(vec![7u8; 100])
        );
        assert_eq!(read_at_most(Cursor::new(vec![7u8; 100]), 99), None);
        assert_eq!(read_at_most(Cursor::new(vec![7u8; 1]), 0), None);
        assert_eq!(read_at_most(Cursor::new(Vec::new()), 0), Some(Vec::new()));
    }

    /// A second name for a file is how another local account lends a plugin
    /// something that account cannot read itself. Creating a link does not
    /// require being able to read its target — not on macOS, and not on Linux
    /// without `fs.protected_hardlinks` — so anyone who can write to the granted
    /// folder can plant one there and let Orivo do the reading.
    ///
    /// Driven through the rule rather than the filesystem because the case that
    /// matters needs a second local account, which a test suite cannot create.
    /// What *is* reproducible is checked below, in
    /// `the_facts_a_refusal_is_made_from_come_from_the_descriptor`: the link
    /// count and owner really are read off the open file.
    #[test]
    fn a_multiply_linked_file_owned_by_another_account_is_refused() {
        let ours = host_account();
        let theirs = ours.wrapping_add(1);
        let entry = |links, owner| EntryFacts {
            file: true,
            byte_size: 32,
            links,
            owner,
        };

        assert_eq!(refuse_entry(&entry(1, ours), ours), None);
        // A deduplicated ROM library is an ordinary thing for a user to have,
        // and every name in it is theirs.
        assert_eq!(refuse_entry(&entry(9, ours), ours), None);
        // One name, another owner: a shared or system folder the user pointed at
        // on purpose. The grant is what authorises this.
        assert_eq!(refuse_entry(&entry(1, theirs), ours), None);
        // A second name for someone else's file is the one shape that is not
        // explained by the user having granted the folder.
        assert_eq!(
            refuse_entry(&entry(2, theirs), ours),
            Some(EntryRefusal::ForeignHardLink)
        );

        assert_eq!(
            refuse_entry(
                &EntryFacts {
                    file: false,
                    ..entry(1, ours)
                },
                ours
            ),
            Some(EntryRefusal::NotAFile)
        );
        assert_eq!(
            refuse_entry(
                &EntryFacts {
                    byte_size: MAX_HOST_FILE_BYTES + 1,
                    ..entry(1, ours)
                },
                ours
            ),
            Some(EntryRefusal::TooLarge)
        );
    }

    /// The rule above is only worth anything if the numbers it judges are the
    /// file's own. A link the test makes itself has two names and this account's
    /// owner, and it stays readable — refusing every hard link would break a
    /// deduplicated library for no security gained.
    #[cfg(unix)]
    #[test]
    fn the_facts_a_refusal_is_made_from_come_from_the_descriptor() {
        let library = FixtureLibrary::new("hard-link");
        fs::hard_link(
            library.games.join("alpha.rom"),
            library.games.join("twin.rom"),
        )
        .unwrap();

        let facts = EntryFacts::of(
            &fs::File::open(library.games.join("twin.rom"))
                .unwrap()
                .metadata()
                .unwrap(),
        );
        assert_eq!(facts.links, 2, "the link count is not the file's own");
        assert_eq!(facts.owner, host_account());

        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert!(harness.prepare("fixture:read-twin").is_ok());
    }

    #[test]
    fn a_file_larger_than_the_host_will_read_is_refused() {
        let library = FixtureLibrary::new("read-big");
        fs::write(
            library.games.join("big.rom"),
            vec![b'x'; MAX_HOST_FILE_BYTES as usize + 1],
        )
        .unwrap();
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert!(matches!(
            harness.prepare("fixture:read-big").unwrap_err(),
            PluginRuntimeError::Plugin {
                code: PluginErrorCode::PermissionDenied,
                ..
            }
        ));
    }

    /// Unix only, and deliberately: creating the link is half of what this
    /// exercises, and a `cfg`-ed-out body would have reported a pass on Windows
    /// without testing anything at all.
    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_a_granted_directory_is_not_listed() {
        let library = FixtureLibrary::new("symlink");
        std::os::unix::fs::symlink(
            library.root.join("secret.txt"),
            library.games.join("zeta.rom"),
        )
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
            page.games.iter().all(|game| game.external_id != "zeta"),
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

    /// Builds a runtime, the fixture and a full grant in one go, for the tests
    /// that need to drive the epoch by hand rather than through `Harness`.
    fn manual(
        limits: PluginLimits,
        library: &FixtureLibrary,
    ) -> (PluginRuntime, PreparedComponent, PluginGrants) {
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
        (runtime, prepared, grants)
    }

    /// Spins the fixture and returns how the host stopped it, ticking the epoch
    /// `ticks` times with `spacing` between each. Bounded, so a deadline that
    /// never fires fails the test instead of hanging it on 2^42 fuel.
    fn spin_under_manual_epoch(
        runtime: &PluginRuntime,
        prepared: &PreparedComponent,
        grants: &PluginGrants,
        ticks: u32,
        spacing: Duration,
    ) -> Result<PluginInvocation, JobError> {
        let handle = runtime
            .submit(
                prepared,
                FIXTURE_PLUGIN_ID,
                grants,
                PluginRequest::PrepareLaunch {
                    profile_id: FIXTURE_PROFILE.into(),
                    game_reference: "fixture:spin".into(),
                },
            )
            .unwrap();
        let ticker = runtime.clone();
        let ticking = thread::spawn(move || {
            for _ in 0..ticks {
                thread::sleep(spacing);
                ticker.tick_epoch();
            }
        });
        let outcome = handle.wait_for(Duration::from_secs(20));
        ticking.join().unwrap();
        match outcome {
            Ok(outcome) => outcome,
            Err(handle) => {
                handle.cancel();
                panic!("nothing stopped the component");
            }
        }
    }

    /// Wasm frames live on the native stack, so `max_wasm_stack` is only a limit
    /// if the thread underneath it is bigger. Wasmtime's documentation says
    /// exhausting the *thread* stack aborts the process, which would take Orivo
    /// down every time Settings → Plugins probed a package like this — and
    /// uninstalling a plugin lives in that panel.
    ///
    /// Read the result honestly: this passes with the worker stack at its default
    /// too, because an overflow taken *inside wasm* hits Wasmtime's guard page and
    /// becomes a trap. The stack size is what protects the case this cannot reach
    /// — an overflow taken in host code, in a host function or a trampoline — and
    /// `a_worker_has_room_for_the_whole_wasm_stack_and_host_frames` is the test
    /// that fails without it. This one is the guest-side regression guard: a
    /// future change that made deep guest recursion abort instead of trap would
    /// fail here.
    #[test]
    fn a_recursing_component_traps_instead_of_aborting_the_process() {
        let library = FixtureLibrary::new("recurse");
        let harness = Harness::new(
            PluginLimits {
                // Generous on both other axes: the stack has to be what stops it.
                interactive_fuel: 1 << 42,
                interactive_deadline: Duration::from_secs(30),
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
                    game_reference: "fixture:recurse".into(),
                },
            )
            .unwrap();
        match handle.wait_for(Duration::from_secs(20)) {
            Ok(outcome) => assert_eq!(
                outcome.unwrap_err(),
                JobError::Runtime(PluginRuntimeError::Trapped)
            ),
            Err(handle) => {
                handle.cancel();
                panic!("the recursing component never came back");
            }
        }
    }

    /// Isolates the tick half. The epoch tick is a whole second, so the wall
    /// clock cannot have run out in the few milliseconds this takes: only the
    /// count of observed ticks can end the call.
    #[test]
    fn the_deadline_runs_out_of_epoch_ticks() {
        let library = FixtureLibrary::new("deadline-ticks");
        let (runtime, prepared, grants) = manual(
            PluginLimits {
                interactive_fuel: 1 << 42,
                interactive_deadline: Duration::from_secs(3),
                epoch_tick: Duration::from_secs(1),
                ..PluginLimits::default()
            },
            &library,
        );
        let started = Instant::now();
        let outcome =
            spin_under_manual_epoch(&runtime, &prepared, &grants, 8, Duration::from_millis(1));
        assert_eq!(
            outcome.unwrap_err(),
            JobError::Runtime(PluginRuntimeError::DeadlineExceeded)
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the wall clock, not the tick count, is what stopped it"
        );
    }

    /// Isolates the wall-clock half. The tick budget is fifty and only ten ticks
    /// happen, so the count cannot reach zero — but they are spaced far enough
    /// apart that the clock passes the deadline. Remove the clock check and this
    /// hangs until the bounded wait fails it.
    #[test]
    fn the_deadline_runs_out_of_wall_clock_between_ticks() {
        let library = FixtureLibrary::new("deadline-clock");
        let (runtime, prepared, grants) = manual(
            PluginLimits {
                interactive_fuel: 1 << 42,
                interactive_deadline: Duration::from_millis(50),
                epoch_tick: Duration::from_millis(1),
                ..PluginLimits::default()
            },
            &library,
        );
        assert_eq!(runtime.limits().ticks(Duration::from_millis(50)), 50);
        let outcome =
            spin_under_manual_epoch(&runtime, &prepared, &grants, 10, Duration::from_millis(10));
        assert_eq!(
            outcome.unwrap_err(),
            JobError::Runtime(PluginRuntimeError::DeadlineExceeded)
        );
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

    fn guard(runtime: &PluginRuntime) -> StoreMemoryGuard {
        StoreMemoryGuard {
            limits: runtime.limits(),
            budget: Arc::clone(&runtime.inner.budget),
            charged: 0,
            hit_limit: None,
        }
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

    /// `memories_per_store` allows four memories, so a limit that looks at one
    /// growth at a time is not a limit on an instance. The guard has to bill the
    /// store's whole footprint.
    #[test]
    fn the_per_instance_ceiling_counts_every_memory_in_the_store() {
        let runtime = PluginRuntime::with_limits(
            PluginLimits {
                instance_memory_bytes: 64 * 1024 * 1024,
                total_memory_bytes: 256 * 1024 * 1024,
                ..PluginLimits::default()
            },
            EpochMode::Manual,
        )
        .unwrap();
        let mut guard = guard(&runtime);

        assert!(guard.memory_growing(0, 40 * 1024 * 1024, None).unwrap());
        // A second memory in the same store, each under the ceiling on its own.
        assert!(!guard.memory_growing(0, 40 * 1024 * 1024, None).unwrap());
        assert_eq!(guard.hit_limit, Some(MemoryLimitKind::Instance));
        assert_eq!(runtime.committed_memory(), 40 * 1024 * 1024);
    }

    /// Wasmtime asks the limiter before it checks the memory's own declared
    /// maximum, so a growth it is about to reject must not be charged. Refusing
    /// it here rather than refunding it later is the whole point: there is no
    /// "growth succeeded" callback, so nothing can tell a later refund whether
    /// it is giving back this growth's charge or the previous one's.
    #[test]
    fn a_growth_past_the_memorys_own_maximum_is_never_charged() {
        let runtime =
            PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Manual).unwrap();
        let mut guard = guard(&runtime);

        for _ in 0..8 {
            assert!(
                !guard
                    .memory_growing(64 * 1024, 32 * 1024 * 1024, Some(64 * 1024))
                    .unwrap()
            );
        }
        assert_eq!(runtime.committed_memory(), 0);
        assert_eq!(guard.charged, 0);
        // And an honest growth still works afterwards.
        assert!(guard.memory_growing(0, 8 * 1024 * 1024, None).unwrap());
        assert_eq!(runtime.committed_memory(), 8 * 1024 * 1024);
    }

    /// Wasmtime calls `memory_grow_failed` *without* calling `memory_growing`
    /// first when the requested size is not representable
    /// (`wasmtime/src/runtime/vm/memory.rs`). A limiter that gives budget back
    /// there hands out a refund for pages it is still holding, and a component
    /// that can reach that path — a second, 64-bit memory will do it — can
    /// repeat the trick until the host is out of memory.
    ///
    /// So the rule this pins is absolute: `memory_grow_failed` never returns
    /// anything to the budget.
    #[test]
    fn a_failure_the_limiter_was_never_asked_about_returns_no_budget() {
        let runtime =
            PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Manual).unwrap();
        let mut guard = guard(&runtime);

        // Nothing charged yet, and a bare failure must still change nothing.
        guard
            .memory_grow_failed(wasmtime::Error::msg("memory growth exceeds address space"))
            .unwrap();
        assert_eq!(runtime.committed_memory(), 0);

        // A real growth, then the same bogus failure on another memory.
        assert!(guard.memory_growing(0, 16 * 1024 * 1024, None).unwrap());
        assert_eq!(runtime.committed_memory(), 16 * 1024 * 1024);
        for _ in 0..8 {
            guard
                .memory_grow_failed(wasmtime::Error::msg("memory growth exceeds address space"))
                .unwrap();
        }
        assert_eq!(
            runtime.committed_memory(),
            16 * 1024 * 1024,
            "a growth that was never asked about was refunded"
        );
        assert_eq!(guard.charged, 16 * 1024 * 1024);
    }

    /// The ABI needs one ordinary 32-bit memory per core module. A 64-bit one is
    /// how a component reaches the address-space overflow path above, so it is
    /// refused before it is ever compiled.
    /// Wasmtime's own adapter modules import two memories when a value crosses
    /// between composed components, and it validates them with the engine's
    /// features and an `expect`. Turning multi-memory off therefore does not
    /// refuse such a component — it *panics*, inside a command that runs on the
    /// main thread. Compiling untrusted bytes must only ever produce a value or a
    /// typed error.
    #[test]
    fn an_unwind_while_compiling_becomes_a_refusal() {
        assert_eq!(without_unwinding(|| Ok(7)), Ok(7));
        assert_eq!(
            without_unwinding::<()>(|| panic!("wasmtime asserted on its own invariant")),
            Err(PluginRuntimeError::InvalidComponent)
        );
    }

    #[test]
    fn a_composed_component_with_two_memories_does_not_panic_the_host() {
        let mut digest = Sha256::new();
        digest.update(COMPOSED_MEMORIES);
        assert_eq!(format!("{:x}", digest.finalize()), COMPOSED_MEMORIES_SHA256);

        let runtime = PluginRuntime::new().unwrap();
        // Either outcome is acceptable; an unwind is not, and is what this
        // catches. A panic here fails the test rather than aborting, because
        // compilation is guarded.
        match runtime.prepare_component(COMPOSED_MEMORIES, COMPOSED_MEMORIES_SHA256) {
            Ok(_) => {}
            Err(error) => assert_eq!(error, PluginRuntimeError::InvalidComponent),
        }
    }

    #[test]
    fn a_component_with_a_64_bit_memory_is_refused() {
        let mut digest = Sha256::new();
        digest.update(MEMORY64);
        assert_eq!(format!("{:x}", digest.finalize()), MEMORY64_SHA256);

        let runtime = PluginRuntime::new().unwrap();
        assert_eq!(
            runtime
                .prepare_component(MEMORY64, MEMORY64_SHA256)
                .unwrap_err(),
            PluginRuntimeError::InvalidComponent
        );
        // And the reference fixture still compiles, so this is not simply off.
        assert!(runtime.prepare_component(FIXTURE, FIXTURE_SHA256).is_ok());
    }

    /// A store that hit the ceiling must give its bytes back when it is dropped,
    /// or the second plugin to run inherits the first one's ceiling.
    #[test]
    fn a_store_that_exhausted_the_ceiling_returns_it_on_the_next_call() {
        let library = FixtureLibrary::new("memory-release");
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
        assert_eq!(harness.runtime.committed_memory(), 0);
        // The ceiling is available again, so an ordinary call still works.
        assert!(harness.prepare("fixture:ok").is_ok());
        assert_eq!(harness.runtime.committed_memory(), 0);
    }

    /// Being refused because *other* plugins had already filled the global
    /// ceiling is not misbehaviour, and must not push an innocent plugin towards
    /// `degraded`.
    #[test]
    fn a_full_host_ceiling_is_not_the_plugins_failure() {
        assert!(!PluginRuntimeError::HostMemoryExhausted.counts_as_plugin_failure());
        assert!(PluginRuntimeError::MemoryLimit.counts_as_plugin_failure());

        let runtime = PluginRuntime::with_limits(
            PluginLimits {
                instance_memory_bytes: 8 * 1024 * 1024,
                total_memory_bytes: 4 * 1024 * 1024,
                ..PluginLimits::default()
            },
            EpochMode::Manual,
        )
        .unwrap();
        let mut guard = guard(&runtime);
        assert!(!guard.memory_growing(0, 6 * 1024 * 1024, None).unwrap());
        assert_eq!(guard.hit_limit, Some(MemoryLimitKind::Host));
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
    fn the_host_refuses_an_intent_that_names_another_runner() {
        let library = FixtureLibrary::new("bad-runner");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert_eq!(
            harness.prepare("fixture:bad-runner").unwrap_err(),
            PluginRuntimeError::InvalidResult("intent runner")
        );
    }

    /// `host-journal` is free of charge in the sense that matters — a plugin may
    /// always explain itself — but not free of accounting. Sharing one ring with
    /// the host's decisions would let a component bury the refusal it just earned
    /// under its own chatter, and leaving the call unmetered would make it the one
    /// import a component can call without limit.
    #[test]
    fn a_plugins_chatter_cannot_bury_the_hosts_decisions() {
        let library = FixtureLibrary::new("chatty");
        let harness = Harness::new(PluginLimits::default(), Some(&library));
        assert!(harness.prepare("fixture:chatty").is_ok());

        let decisions = harness.runtime.journal().entries();
        assert!(
            decisions
                .iter()
                .any(|entry| entry.decision == "scope-refused"),
            "the refusal the plugin earned was evicted by its own logging"
        );

        let messages = harness.runtime.journal().plugin_messages();
        assert!(!messages.is_empty());
        // The plugin's own text never lands in the ring the host keeps its
        // decisions in. This is the assertion that fails if the two are merged
        // again — counting messages cannot fail, because the ring holds 256
        // whether the call is metered or not.
        assert!(
            decisions.iter().all(|entry| entry.decision != "plugin-log"),
            "plugin text was written into the host's decision ring"
        );
        // The fixture asks a thousand times against a budget of 256, so the
        // budget line is present exactly when `log` is metered — and exactly
        // once, because reporting each refusal was its own way of flooding the
        // ring this test is about.
        assert_eq!(
            decisions
                .iter()
                .filter(|entry| entry.decision == "host-call-budget")
                .count(),
            1,
            "host-journal was either unmetered or reported its refusal repeatedly"
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
                    vec![
                        candidate("prov", "one", "One"),
                        candidate("prov", "two", "Two")
                    ],
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
                    vec![
                        candidate("prov", "one", "One"),
                        candidate("prov", "one", "Again")
                    ],
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
                page(
                    vec![candidate("prov", "one", "One")],
                    Some("../escape"),
                    false
                ),
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

    /// The grammar is checked here rather than through the filesystem because
    /// the names that matter most are the ones that only escape on Windows, and
    /// CI runs `cargo check` there, not `cargo test`. A drive prefix makes
    /// `Path::join` throw the grant away entirely.
    #[test]
    fn an_entry_name_is_one_ordinary_component_or_nothing() {
        assert!(valid_entry_name("alpha.rom"));
        assert!(valid_entry_name("Alpha Quest (1994).rom"));

        assert!(!valid_entry_name(""));
        assert!(!valid_entry_name("."));
        assert!(!valid_entry_name(".."));
        assert!(!valid_entry_name("../secret.txt"));
        assert!(!valid_entry_name("nested/alpha.rom"));
        assert!(!valid_entry_name("/etc/passwd"));
        assert!(!valid_entry_name("alpha\\beta"));
        assert!(!valid_entry_name("\\\\server\\share\\secret.txt"));
        assert!(!valid_entry_name("alpha\u{0}.rom"));
        assert!(!valid_entry_name("alpha\u{7}.rom"));
        assert!(!valid_entry_name(&"a".repeat(MAX_ENTRY_NAME_BYTES + 1)));

        // A Windows drive prefix: `root.join("D:secrets.txt")` is
        // `D:secrets.txt`, and `C:x` resolves against that drive's own current
        // directory. Neither is inside the grant.
        assert!(!valid_entry_name("D:secrets.txt"));
        assert!(!valid_entry_name("C:x"));
        assert!(!valid_entry_name("C:\\Windows\\System32\\config\\SAM"));

        // Windows device names. `read_file(grant, "CON")` opens the console
        // rather than a file in the grant, and it blocks the worker exactly like
        // a FIFO does; `COM1` opens a serial port. Win32 ignores everything from
        // the first dot and strips trailing spaces and dots, so every spelling
        // below is the same device.
        for device in [
            "CON", "con", "Con", "PRN", "AUX", "NUL", "nul", "COM1", "com9", "LPT1", "lpt9",
            "COM0", "LPT0", "CONIN$", "CONOUT$",
        ] {
            assert!(!valid_entry_name(device), "{device} was accepted");
        }
        assert!(!valid_entry_name("CON.txt"));
        assert!(!valid_entry_name("nul.rom"));
        assert!(!valid_entry_name("CON."));
        assert!(!valid_entry_name("com1 "));
        assert!(!valid_entry_name("..."));
        // Win32 trims the spaces before the dot as well as at the end.
        assert!(!valid_entry_name("NUL .rom"));
        assert!(!valid_entry_name("CON   .txt"));
        // And it folds the superscript digits onto ASCII ones.
        assert!(!valid_entry_name("COM\u{b9}"));
        assert!(!valid_entry_name("COM\u{b2}.rom"));
        assert!(!valid_entry_name("lpt\u{b3}"));

        // And names that merely start the same way are ordinary files.
        assert!(valid_entry_name("console.rom"));
        assert!(valid_entry_name("communication.rom"));
        assert!(valid_entry_name("nullify.rom"));
        assert!(valid_entry_name("COM.rom"));
        assert!(valid_entry_name("LPT10.rom"));
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

        // Past the failure threshold, so this distinguishes "a cancellation does
        // not count" from "it counts, and three have not happened yet".
        for _ in 0..DEFAULT_MAX_CONSECUTIVE_FAILURES {
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
                .expect("a cancelled plugin is still accepted");
            let handle = handle
                .wait_for(Duration::from_millis(30))
                .err()
                .expect("an endless component is still running after 30ms");
            handle.cancel();
            assert_eq!(handle.wait().unwrap_err(), JobError::Cancelled);
        }
        let health = harness.runtime.scheduler().health(FIXTURE_PLUGIN_ID);
        assert!(!health.degraded, "cancellations were blamed on the plugin");
        assert_eq!(health.consecutive_failures, 0);
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

        let prepared_launch = harness
            .call_with(launch.clone(), &Arc::new(AtomicBool::new(false)))
            .unwrap();
        let prepared_discover = harness
            .call_with(discover, &Arc::new(AtomicBool::new(false)))
            .unwrap();
        let first_launch = on_a_host_sized_thread(|| {
            harness.runtime.invoke(
                &cold,
                FIXTURE_PLUGIN_ID,
                &harness.grants,
                next_correlation_id(),
                &Arc::new(AtomicBool::new(false)),
                &launch,
            )
        })
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
