//! The bounded queue in front of the plugin host.
//!
//! Nothing on Orivo's rendering path may call a plugin, so every invocation
//! becomes a job with a correlation id, a state, a cancel token and an end. The
//! scheduler is what makes that true rather than aspirational:
//!
//! * global concurrency is bounded, and one plugin runs one job at a time, so a
//!   chatty extension cannot occupy every worker;
//! * a per-plugin queue has a depth, and a full queue answers `Busy` instead of
//!   growing — back-pressure is a typed refusal, not an allocation;
//! * cancellation reaches a job in the queue *and* a call already inside
//!   Wasmtime, through the same token the epoch callback reads;
//! * after repeated failures a plugin is parked in `degraded` and only an
//!   explicit resume restarts it. There is no retry loop anywhere in this file.
//!
//! The scheduler never knows what a job does. It takes a closure returning the
//! caller's own type, which is what lets its tests exercise back-pressure,
//! cancellation and the degraded transition without a WebAssembly component.

use crate::plugin_runtime::{
    CorrelationId, DEFAULT_MAX_CONSECUTIVE_FAILURES, PluginJournal, PluginRuntimeError,
    next_correlation_id,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Finished jobs whose state stays queryable. Bounded because a handle may
/// outlive interest in it and the map must not become a leak.
const MAX_RETAINED_JOBS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerLimits {
    /// Plugin work is never the reason Orivo feels slow, so it gets a small
    /// fixed share of the machine rather than a share of the cores.
    pub max_concurrency: usize,
    /// How many jobs one plugin may have waiting. Deep enough for an import
    /// that pages, shallow enough that `Busy` arrives while the user is still
    /// looking at the thing they asked for.
    pub queue_depth_per_plugin: usize,
    pub max_consecutive_failures: u32,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_concurrency: 2,
            queue_depth_per_plugin: 8,
            max_consecutive_failures: DEFAULT_MAX_CONSECUTIVE_FAILURES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// Why the scheduler would not take a job. None of these are retried for the
/// caller: `Busy` means ask again later, `Degraded` means a human has to resume
/// the plugin first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    Busy {
        queued: usize,
    },
    Degraded {
        consecutive_failures: u32,
    },
    ShuttingDown,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { .. } => write!(
                formatter,
                "This plugin already has as much work queued as Orivo will hold."
            ),
            Self::Degraded { .. } => write!(
                formatter,
                "Orivo paused this plugin after repeated failures. Resume it to try again."
            ),
            Self::ShuttingDown => write!(formatter, "Orivo is closing its plugin worker."),
        }
    }
}

impl std::error::Error for SubmitError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    Cancelled,
    Runtime(PluginRuntimeError),
    /// The worker went away before the job ran — shutdown, not misbehaviour.
    Abandoned,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(formatter, "The plugin job was cancelled."),
            Self::Runtime(error) => write!(formatter, "{error}"),
            Self::Abandoned => write!(formatter, "The plugin job did not run."),
        }
    }
}

impl std::error::Error for JobError {}

/// What a running job is allowed to know about itself.
#[derive(Debug, Clone)]
pub struct JobContext {
    correlation_id: CorrelationId,
    cancel: Arc<AtomicBool>,
}

impl JobContext {
    pub fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// The same flag the host's epoch callback reads, so a long call inside
    /// Wasmtime and a plain Rust loop are cancelled by one mechanism.
    pub fn cancel_token(&self) -> &Arc<AtomicBool> {
        &self.cancel
    }

    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// What the scheduler learns from a finished job. Deliberately not the payload:
/// the health of a plugin must not depend on what its results contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobReport {
    Done,
    Cancelled,
    /// The host refused the call — an ungranted capability, a closing worker.
    /// Not the plugin's fault, so it does not count towards `degraded`.
    Refused,
    Failed,
}

/// Handing the caller its value is deliberately separated from running the job,
/// so the worker can commit the plugin's bookkeeping *before* waking whoever is
/// waiting. Otherwise `wait()` can return while the failure that caused it has
/// not yet been counted, and a caller reading the plugin's health immediately
/// afterwards sees the state from before its own job.
type Delivery = Box<dyn FnOnce() + Send + 'static>;

struct Task {
    correlation_id: CorrelationId,
    plugin_id: String,
    cancel: Arc<AtomicBool>,
    work: Box<dyn FnOnce(&JobContext) -> (JobReport, Delivery) + Send + 'static>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PluginHealthState {
    pub queued: usize,
    pub running: usize,
    pub consecutive_failures: u32,
    pub degraded: bool,
}

#[derive(Debug, Default)]
struct PluginRunState {
    queued: usize,
    running: usize,
    consecutive_failures: u32,
    degraded: bool,
}

struct SchedulerState {
    queue: VecDeque<Task>,
    plugins: BTreeMap<String, PluginRunState>,
    jobs: BTreeMap<CorrelationId, JobState>,
    retired: VecDeque<CorrelationId>,
    /// Cancel tokens for the jobs currently inside the host. Keeping them here
    /// rather than on the worker is what lets `cancel` reach a call that has
    /// already entered Wasmtime.
    in_flight: BTreeMap<CorrelationId, Arc<AtomicBool>>,
    running: usize,
    stopping: bool,
}

struct Inner {
    limits: SchedulerLimits,
    journal: Arc<PluginJournal>,
    state: Mutex<SchedulerState>,
    wake: Condvar,
}

/// One job, from the caller's side.
pub struct JobHandle<T> {
    correlation_id: CorrelationId,
    inner: Arc<Inner>,
    result: mpsc::Receiver<Result<T, JobError>>,
}

impl<T> std::fmt::Debug for JobHandle<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobHandle")
            .field("correlation_id", &self.correlation_id)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl<T> JobHandle<T> {
    #[allow(dead_code)]
    pub fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub fn state(&self) -> JobState {
        self.inner.job_state(self.correlation_id)
    }

    /// Cancels the job wherever it is. A queued job is dropped before it runs;
    /// a running one is interrupted at the next epoch tick.
    pub fn cancel(&self) {
        self.inner.cancel(self.correlation_id);
    }

    #[allow(dead_code)]
    pub fn wait(self) -> Result<T, JobError> {
        match self.result.recv() {
            Ok(outcome) => outcome,
            Err(_) => Err(self.inner.abandoned_reason(self.correlation_id)),
        }
    }

    /// Waits up to `budget`, handing the job back if it has not finished. This
    /// is how an interactive caller honours the 150 ms display budget without
    /// giving up on the job it started.
    pub fn wait_for(self, budget: Duration) -> Result<Result<T, JobError>, Self> {
        match self.result.recv_timeout(budget) {
            Ok(outcome) => Ok(outcome),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Ok(Err(self.inner.abandoned_reason(self.correlation_id)))
            }
        }
    }
}

pub struct PluginScheduler {
    inner: Arc<Inner>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for PluginScheduler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginScheduler")
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

impl PluginScheduler {
    pub fn new(limits: SchedulerLimits, journal: Arc<PluginJournal>) -> Self {
        let inner = Arc::new(Inner {
            limits,
            journal,
            state: Mutex::new(SchedulerState {
                queue: VecDeque::new(),
                plugins: BTreeMap::new(),
                jobs: BTreeMap::new(),
                retired: VecDeque::new(),
                in_flight: BTreeMap::new(),
                running: 0,
                stopping: false,
            }),
            wake: Condvar::new(),
        });
        let workers = (0..limits.max_concurrency.max(1))
            .filter_map(|index| {
                let inner = Arc::clone(&inner);
                thread::Builder::new()
                    .name(format!("orivo-plugin-{index}"))
                    .spawn(move || inner.work())
                    .ok()
            })
            .collect();
        Self {
            inner,
            workers: Mutex::new(workers),
        }
    }

    #[allow(dead_code)]
    pub fn limits(&self) -> SchedulerLimits {
        self.inner.limits
    }

    /// Queues one call. The closure runs on a worker thread and receives the
    /// job's own cancel token, which it is expected to hand to the host so a
    /// cancellation reaches the component itself.
    pub fn submit<T, F>(&self, plugin_id: &str, work: F) -> Result<JobHandle<T>, SubmitError>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> Result<T, PluginRuntimeError> + Send + 'static,
    {
        let correlation_id = next_correlation_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let journal = Arc::clone(&self.inner.journal);
        let plugin_for_work = plugin_id.to_owned();
        let task = Task {
            correlation_id,
            plugin_id: plugin_id.to_owned(),
            cancel: Arc::clone(&cancel),
            work: Box::new(move |context| match work(context) {
                Ok(value) => (
                    JobReport::Done,
                    Box::new(move || {
                        let _ = sender.send(Ok(value));
                    }) as Delivery,
                ),
                Err(PluginRuntimeError::Cancelled) => (
                    JobReport::Cancelled,
                    Box::new(move || {
                        let _ = sender.send(Err(JobError::Cancelled));
                    }) as Delivery,
                ),
                Err(error) => {
                    let report = if error.counts_as_plugin_failure() {
                        JobReport::Failed
                    } else {
                        JobReport::Refused
                    };
                    journal.record(
                        context.correlation_id(),
                        &plugin_for_work,
                        "job-failed",
                        error.to_string(),
                    );
                    (
                        report,
                        Box::new(move || {
                            let _ = sender.send(Err(JobError::Runtime(error)));
                        }) as Delivery,
                    )
                }
            }),
        };

        {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| SubmitError::ShuttingDown)?;
            if state.stopping {
                return Err(SubmitError::ShuttingDown);
            }
            let plugin = state.plugins.entry(plugin_id.to_owned()).or_default();
            if plugin.degraded {
                let failures = plugin.consecutive_failures;
                self.inner.journal.record(
                    correlation_id,
                    plugin_id,
                    "submit-refused",
                    "the plugin is paused after repeated failures",
                );
                return Err(SubmitError::Degraded {
                    consecutive_failures: failures,
                });
            }
            if plugin.queued >= self.inner.limits.queue_depth_per_plugin {
                let queued = plugin.queued;
                self.inner.journal.record(
                    correlation_id,
                    plugin_id,
                    "submit-refused",
                    format!("the plugin queue is full ({queued} waiting)"),
                );
                return Err(SubmitError::Busy { queued });
            }
            plugin.queued += 1;
            state.jobs.insert(correlation_id, JobState::Queued);
            state.queue.push_back(task);
        }
        self.inner.wake.notify_all();
        Ok(JobHandle {
            correlation_id,
            inner: Arc::clone(&self.inner),
            result: receiver,
        })
    }

    #[allow(dead_code)]
    pub fn health(&self, plugin_id: &str) -> PluginHealthState {
        self.inner
            .state
            .lock()
            .ok()
            .and_then(|state| {
                state.plugins.get(plugin_id).map(|plugin| PluginHealthState {
                    queued: plugin.queued,
                    running: plugin.running,
                    consecutive_failures: plugin.consecutive_failures,
                    degraded: plugin.degraded,
                })
            })
            .unwrap_or_default()
    }

    /// Takes a plugin out of `degraded`. Explicit on purpose: the plan asks for
    /// a resume button, not a timer that re-enables a broken extension behind
    /// the user's back.
    #[allow(dead_code)]
    pub fn resume(&self, plugin_id: &str) {
        if let Ok(mut state) = self.inner.state.lock()
            && let Some(plugin) = state.plugins.get_mut(plugin_id)
        {
            plugin.degraded = false;
            plugin.consecutive_failures = 0;
            self.inner
                .journal
                .record(next_correlation_id(), plugin_id, "resumed", "by request");
        }
    }
}

impl Drop for PluginScheduler {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.stopping = true;
            state.queue.clear();
        }
        self.inner.wake.notify_all();
        let workers = self
            .workers
            .lock()
            .map(|mut workers| std::mem::take(&mut *workers))
            .unwrap_or_default();
        for worker in workers {
            let _ = worker.join();
        }
    }
}

impl Inner {
    fn job_state(&self, correlation_id: CorrelationId) -> JobState {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.jobs.get(&correlation_id).copied())
            // A job the scheduler no longer remembers finished long enough ago
            // that its slot was retired; reporting it as done is truthful and
            // keeps the retained-job map bounded.
            .unwrap_or(JobState::Done)
    }

    fn abandoned_reason(&self, correlation_id: CorrelationId) -> JobError {
        match self.job_state(correlation_id) {
            JobState::Cancelled => JobError::Cancelled,
            _ => JobError::Abandoned,
        }
    }

    fn cancel(&self, correlation_id: CorrelationId) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(current) = state.jobs.get(&correlation_id).copied() else {
            return;
        };
        match current {
            JobState::Queued => {
                if let Some(index) = state
                    .queue
                    .iter()
                    .position(|task| task.correlation_id == correlation_id)
                    && let Some(task) = state.queue.remove(index)
                {
                    task.cancel.store(true, Ordering::Relaxed);
                    if let Some(plugin) = state.plugins.get_mut(&task.plugin_id) {
                        plugin.queued = plugin.queued.saturating_sub(1);
                    }
                    self.journal.record(
                        correlation_id,
                        &task.plugin_id,
                        "cancelled",
                        "dropped before it ran",
                    );
                    // Dropping the task drops the closure that owns the result
                    // sender, so a waiting caller is woken by the disconnect
                    // and reads this state rather than blocking forever.
                    state.set_job(correlation_id, JobState::Cancelled);
                }
            }
            JobState::Running => {
                // The running task owns the only other handle to this token;
                // the host's epoch callback picks it up within one tick.
                if let Some(cancel) = state.running_cancel(correlation_id) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }

    /// Picks the next runnable task: the oldest queued job whose plugin has
    /// nothing in flight. Scanning is bounded by the queue, which is itself
    /// bounded per plugin, so this cannot degrade into a search.
    fn take_next(&self, state: &mut SchedulerState) -> Option<Task> {
        if state.running >= self.limits.max_concurrency.max(1) {
            return None;
        }
        let index = state.queue.iter().position(|task| {
            state
                .plugins
                .get(&task.plugin_id)
                .is_none_or(|plugin| plugin.running == 0)
        })?;
        let task = state.queue.remove(index)?;
        let plugin = state.plugins.entry(task.plugin_id.clone()).or_default();
        plugin.queued = plugin.queued.saturating_sub(1);
        plugin.running += 1;
        state.running += 1;
        state.set_job(task.correlation_id, JobState::Running);
        state
            .in_flight
            .insert(task.correlation_id, Arc::clone(&task.cancel));
        Some(task)
    }

    fn work(&self) {
        loop {
            let task = {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                let mut next = self.take_next(&mut state);
                while next.is_none() {
                    if state.stopping {
                        return;
                    }
                    let Ok(waited) = self.wake.wait(state) else {
                        return;
                    };
                    state = waited;
                    next = self.take_next(&mut state);
                }
                next
            };
            let Some(task) = task else {
                return;
            };
            let context = JobContext {
                correlation_id: task.correlation_id,
                cancel: Arc::clone(&task.cancel),
            };
            let plugin_id = task.plugin_id.clone();
            let correlation_id = task.correlation_id;
            let (report, deliver) = (task.work)(&context);
            self.finish(&plugin_id, correlation_id, report);
            deliver();
        }
    }

    fn finish(&self, plugin_id: &str, correlation_id: CorrelationId, report: JobReport) {
        let mut degraded_now = false;
        if let Ok(mut state) = self.state.lock() {
            state.running = state.running.saturating_sub(1);
            state.in_flight.remove(&correlation_id);
            if let Some(plugin) = state.plugins.get_mut(plugin_id) {
                plugin.running = plugin.running.saturating_sub(1);
                match report {
                    JobReport::Done => plugin.consecutive_failures = 0,
                    JobReport::Failed => {
                        plugin.consecutive_failures += 1;
                        if plugin.consecutive_failures >= self.limits.max_consecutive_failures {
                            plugin.degraded = true;
                            degraded_now = true;
                        }
                    }
                    JobReport::Cancelled | JobReport::Refused => {}
                }
            }
            let next = match report {
                JobReport::Done => JobState::Done,
                JobReport::Cancelled => JobState::Cancelled,
                JobReport::Failed | JobReport::Refused => JobState::Failed,
            };
            state.set_job(correlation_id, next);
        }
        if degraded_now {
            self.journal.record(
                correlation_id,
                plugin_id,
                "degraded",
                "paused after repeated failures; a resume is required",
            );
        }
        self.wake.notify_all();
    }
}

impl SchedulerState {
    /// Keeps the queryable job map bounded. A job's state stays readable for
    /// the next few hundred jobs, which is longer than any handle lives.
    fn set_job(&mut self, correlation_id: CorrelationId, next: JobState) {
        self.jobs.insert(correlation_id, next);
        if matches!(
            next,
            JobState::Done | JobState::Failed | JobState::Cancelled
        ) {
            self.retired.push_back(correlation_id);
            while self.retired.len() > MAX_RETAINED_JOBS {
                if let Some(expired) = self.retired.pop_front() {
                    self.jobs.remove(&expired);
                }
            }
        }
    }

    fn running_cancel(&self, correlation_id: CorrelationId) -> Option<&Arc<AtomicBool>> {
        self.in_flight.get(&correlation_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_manifest::PluginCapability;
    use std::sync::atomic::AtomicUsize;

    const PLUGIN: &str = "com.orivo.fixture-runner";
    const OTHER: &str = "com.orivo.other-runner";

    fn scheduler(limits: SchedulerLimits) -> PluginScheduler {
        PluginScheduler::new(limits, Arc::new(PluginJournal::default()))
    }

    /// Blocks a worker until the test releases it, which is how every queue and
    /// concurrency assertion below gets a stable "one job is in flight" state.
    #[derive(Clone)]
    struct Gate(Arc<(Mutex<bool>, Condvar)>);

    impl Gate {
        fn new() -> Self {
            Self(Arc::new((Mutex::new(false), Condvar::new())))
        }

        fn wait(&self) {
            let mut open = self.0.0.lock().unwrap();
            while !*open {
                open = self.0.1.wait(open).unwrap();
            }
        }

        fn open(&self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }

    /// Spins until `condition` holds. The scheduler hands work to threads, so a
    /// test cannot assert on a transition the instant it asks for one.
    fn eventually(condition: impl Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn runs_a_job_and_hands_back_its_value() {
        let scheduler = scheduler(SchedulerLimits::default());
        let handle = scheduler.submit(PLUGIN, |_| Ok(41 + 1)).unwrap();
        let correlation_id = handle.correlation_id();
        assert_eq!(handle.wait().unwrap(), 42);
        assert_eq!(scheduler.health(PLUGIN).consecutive_failures, 0);
        assert!(correlation_id.0 > 0);
    }

    #[test]
    fn a_full_queue_answers_busy_instead_of_growing() {
        let scheduler = scheduler(SchedulerLimits {
            max_concurrency: 1,
            queue_depth_per_plugin: 2,
            ..SchedulerLimits::default()
        });
        let gate = Gate::new();
        let held = gate.clone();
        let running = scheduler
            .submit(PLUGIN, move |_| {
                held.wait();
                Ok(())
            })
            .unwrap();
        assert!(eventually(|| scheduler.health(PLUGIN).running == 1));

        let queued = [
            scheduler.submit(PLUGIN, |_| Ok(())).unwrap(),
            scheduler.submit(PLUGIN, |_| Ok(())).unwrap(),
        ];
        assert_eq!(
            scheduler.submit::<(), _>(PLUGIN, |_| Ok(())).unwrap_err(),
            SubmitError::Busy { queued: 2 }
        );

        gate.open();
        running.wait().unwrap();
        for handle in queued {
            handle.wait().unwrap();
        }
        // Back-pressure is a refusal, not a delay: once the queue drains the
        // same submission is accepted again.
        assert!(scheduler.submit::<(), _>(PLUGIN, |_| Ok(())).is_ok());
    }

    #[test]
    fn cancelling_a_queued_job_never_runs_it() {
        let scheduler = scheduler(SchedulerLimits {
            max_concurrency: 1,
            ..SchedulerLimits::default()
        });
        let gate = Gate::new();
        let held = gate.clone();
        let running = scheduler
            .submit(PLUGIN, move |_| {
                held.wait();
                Ok(())
            })
            .unwrap();
        assert!(eventually(|| scheduler.health(PLUGIN).running == 1));

        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let queued = scheduler
            .submit(PLUGIN, move |_| {
                flag.store(true, Ordering::Relaxed);
                Ok(())
            })
            .unwrap();
        assert_eq!(queued.state(), JobState::Queued);
        queued.cancel();
        assert_eq!(queued.state(), JobState::Cancelled);

        gate.open();
        running.wait().unwrap();
        assert_eq!(queued.wait().unwrap_err(), JobError::Cancelled);
        assert!(!ran.load(Ordering::Relaxed), "a cancelled job still ran");
        assert_eq!(scheduler.health(PLUGIN).queued, 0);
    }

    #[test]
    fn cancelling_a_running_job_reaches_the_token_it_is_holding() {
        let scheduler = scheduler(SchedulerLimits::default());
        let started = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&started);
        let handle = scheduler
            .submit::<(), _>(PLUGIN, move |context| {
                observed.store(true, Ordering::Relaxed);
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(PluginRuntimeError::Cancelled)
            })
            .unwrap();
        assert!(eventually(|| started.load(Ordering::Relaxed)));
        handle.cancel();
        assert_eq!(handle.wait().unwrap_err(), JobError::Cancelled);
        // A user's cancellation is not a plugin failure.
        assert_eq!(scheduler.health(PLUGIN).consecutive_failures, 0);
    }

    #[test]
    fn repeated_failures_park_the_plugin_until_it_is_resumed() {
        let scheduler = scheduler(SchedulerLimits {
            max_consecutive_failures: 3,
            ..SchedulerLimits::default()
        });
        for _ in 0..3 {
            let handle = scheduler
                .submit::<(), _>(PLUGIN, |_| Err(PluginRuntimeError::DeadlineExceeded))
                .unwrap();
            assert_eq!(
                handle.wait().unwrap_err(),
                JobError::Runtime(PluginRuntimeError::DeadlineExceeded)
            );
        }
        let health = scheduler.health(PLUGIN);
        assert!(health.degraded);
        assert_eq!(health.consecutive_failures, 3);
        assert_eq!(
            scheduler.submit::<(), _>(PLUGIN, |_| Ok(())).unwrap_err(),
            SubmitError::Degraded {
                consecutive_failures: 3
            }
        );
        // No timer and no retry loop: a human resumes it.
        scheduler.resume(PLUGIN);
        assert!(!scheduler.health(PLUGIN).degraded);
        assert_eq!(scheduler.submit(PLUGIN, |_| Ok(7)).unwrap().wait().unwrap(), 7);
    }

    #[test]
    fn a_success_clears_the_failures_before_it() {
        let scheduler = scheduler(SchedulerLimits {
            max_consecutive_failures: 3,
            ..SchedulerLimits::default()
        });
        for _ in 0..2 {
            let _ = scheduler
                .submit::<(), _>(PLUGIN, |_| Err(PluginRuntimeError::Trapped))
                .unwrap()
                .wait();
        }
        assert_eq!(scheduler.health(PLUGIN).consecutive_failures, 2);
        scheduler.submit(PLUGIN, |_| Ok(())).unwrap().wait().unwrap();
        assert_eq!(scheduler.health(PLUGIN).consecutive_failures, 0);
        assert!(!scheduler.health(PLUGIN).degraded);
    }

    #[test]
    fn a_refused_capability_never_parks_the_plugin() {
        let scheduler = scheduler(SchedulerLimits {
            max_consecutive_failures: 2,
            ..SchedulerLimits::default()
        });
        for _ in 0..4 {
            let _ = scheduler
                .submit::<(), _>(PLUGIN, |_| {
                    Err(PluginRuntimeError::CapabilityUndeclared(
                        PluginCapability::FilesRead,
                    ))
                })
                .unwrap()
                .wait();
        }
        let health = scheduler.health(PLUGIN);
        assert!(!health.degraded, "a host refusal was blamed on the plugin");
        assert_eq!(health.consecutive_failures, 0);
    }

    #[test]
    fn one_plugin_cannot_occupy_every_worker() {
        let scheduler = scheduler(SchedulerLimits {
            max_concurrency: 2,
            queue_depth_per_plugin: 4,
            ..SchedulerLimits::default()
        });
        let gate = Gate::new();
        let mut greedy = Vec::new();
        for _ in 0..3 {
            let held = gate.clone();
            greedy.push(
                scheduler
                    .submit(PLUGIN, move |_| {
                        held.wait();
                        Ok(())
                    })
                    .unwrap(),
            );
        }
        assert!(eventually(|| scheduler.health(PLUGIN).running == 1));

        // The second worker is free even though one plugin has three jobs, so
        // another plugin's work is not stuck behind them.
        let polite = scheduler.submit(OTHER, |_| Ok(5)).unwrap();
        assert_eq!(polite.wait().unwrap(), 5);
        assert_eq!(scheduler.health(PLUGIN).running, 1);

        gate.open();
        for handle in greedy {
            handle.wait().unwrap();
        }
    }

    #[test]
    fn wait_for_hands_the_job_back_when_the_display_budget_expires() {
        let scheduler = scheduler(SchedulerLimits::default());
        let gate = Gate::new();
        let held = gate.clone();
        let handle = scheduler
            .submit(PLUGIN, move |_| {
                held.wait();
                Ok(9)
            })
            .unwrap();
        let handle = handle
            .wait_for(Duration::from_millis(20))
            .err()
            .expect("the job should still be running");
        gate.open();
        assert_eq!(handle.wait().unwrap(), 9);
    }

    /// Shutting down must not leave a caller waiting on a job no worker will
    /// ever take. The scheduler is dropped while one job is still in flight, so
    /// the queued one is cleared exactly as it is when Orivo closes.
    #[test]
    fn a_dropped_scheduler_abandons_what_it_never_ran() {
        let scheduler = scheduler(SchedulerLimits {
            max_concurrency: 1,
            queue_depth_per_plugin: 4,
            ..SchedulerLimits::default()
        });
        let gate = Gate::new();
        let ran = Arc::new(AtomicUsize::new(0));

        let held = gate.clone();
        let counter = Arc::clone(&ran);
        let running = scheduler
            .submit(PLUGIN, move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
                held.wait();
                Ok(())
            })
            .unwrap();
        assert!(eventually(|| scheduler.health(PLUGIN).running == 1));

        let counter = Arc::clone(&ran);
        let queued = scheduler
            .submit(PLUGIN, move |_| {
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .unwrap();

        // The drop clears the queue before it joins the worker, which is still
        // inside the first job; releasing the gate afterwards lets it exit.
        let closing = thread::spawn(move || drop(scheduler));
        thread::sleep(Duration::from_millis(20));
        gate.open();
        running.wait().unwrap();
        closing.join().unwrap();

        assert_eq!(queued.wait().unwrap_err(), JobError::Abandoned);
        assert_eq!(ran.load(Ordering::Relaxed), 1);
    }
}
