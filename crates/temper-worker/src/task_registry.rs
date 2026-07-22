//! Explicit ownership and shutdown coordination for worker-spawned tasks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use temper_protocol_worker::{
    MAX_SHUTDOWN_BLOCKERS, ShutdownBlocker, ShutdownBlockerKind, ShutdownEscalationStage,
};

use crate::executor::{AttemptFence, JobCancellation, JobCancellationRequest};

mod component_tasks;
pub use component_tasks::WorkerComponentTaskKind;
pub(crate) use component_tasks::WorkerComponentTasks;

/// Component-level shutdown semantics applied to every active attempt.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WorkerShutdown {
    /// Fence publication, request cooperative cancellation, then use the
    /// configured TERM/KILL escalation deadlines.
    Graceful,
    /// Preserve the durable claim while immediately requesting hard cleanup.
    Crash,
}

/// Worker-owned join state for one active attempt task.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ActiveJobJoinState {
    Registered,
    Running,
    CancellationRequested,
    ForcedTerminationRequested,
    HardKillRequested,
    CleanupPending,
    Joined,
}

/// Stable worker/assignment identity returned by bounded shutdown.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorkerAttemptIdentity {
    pub worker_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub generation: u64,
}

/// Exact structured conditions that remained unresolved at the shutdown
/// deadline. The alias keeps the worker API descriptive while sharing one DTO
/// with daemon and standalone diagnostics.
pub type WorkerShutdownBlocker = ShutdownBlocker;

/// Result of a bounded worker join. Unresolved entries remain registered and
/// fenced; this report is evidence only and never fabricates local quiescence.
#[derive(Clone, Debug, Default)]
pub struct WorkerShutdownReport {
    pub joined_attempts: Vec<WorkerAttemptIdentity>,
    pub unresolved_blockers: Vec<WorkerShutdownBlocker>,
}

#[derive(Clone, Debug)]
struct ObservedShutdownBlocker {
    blocker: ShutdownBlocker,
    observed_at: Instant,
}

/// The complete worker-local ownership record for one daemon assignment.
#[derive(Clone, Debug)]
pub struct ActiveJobTask {
    job_id: String,
    attempt_id: String,
    generation: u64,
    fence: AttemptFence,
    cancellation: JobCancellation,
    join_state: ActiveJobJoinState,
    shutdown_blockers: Vec<ObservedShutdownBlocker>,
}

impl ActiveJobTask {
    pub fn new(
        job_id: impl Into<String>,
        attempt_id: impl Into<String>,
        generation: u64,
        fence: AttemptFence,
        cancellation: JobCancellation,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            attempt_id: attempt_id.into(),
            generation,
            fence,
            cancellation,
            join_state: ActiveJobJoinState::Registered,
            shutdown_blockers: Vec::new(),
        }
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn fence(&self) -> &AttemptFence {
        &self.fence
    }

    pub fn cancellation(&self) -> &JobCancellation {
        &self.cancellation
    }

    pub fn join_state(&self) -> ActiveJobJoinState {
        self.join_state
    }

    pub fn identity(&self, worker_id: &str) -> WorkerAttemptIdentity {
        WorkerAttemptIdentity {
            worker_id: worker_id.to_string(),
            job_id: self.job_id.clone(),
            attempt_id: self.attempt_id.clone(),
            generation: self.generation,
        }
    }

    pub(crate) fn shutdown_blockers(
        &self,
        worker_id: &str,
        escalation_stage: ShutdownEscalationStage,
        deadline: Instant,
    ) -> Vec<ShutdownBlocker> {
        let now = Instant::now();
        let remaining = duration_millis(deadline.saturating_duration_since(now));
        let mut blockers = self
            .shutdown_blockers
            .iter()
            .map(|observed| {
                let mut blocker = observed.blocker.clone().with_identity(
                    Some(worker_id),
                    Some(&self.job_id),
                    Some(&self.attempt_id),
                );
                blocker.escalation_stage = escalation_stage;
                let first_seen_millis = blocker.first_seen_millis;
                blocker.with_timing(
                    first_seen_millis,
                    duration_millis(now.saturating_duration_since(observed.observed_at)),
                    remaining,
                )
            })
            .collect::<Vec<_>>();

        let emergency = self
            .cancellation
            .emergency_termination_registry()
            .snapshot();
        for boundary in emergency.boundaries() {
            if blockers.len() >= MAX_SHUTDOWN_BLOCKERS.saturating_sub(1) {
                break;
            }
            let root_pid = (boundary.root_pid() != 0).then_some(boundary.root_pid());
            blockers.push(
                ShutdownBlocker::new(
                    ShutdownBlockerKind::Containment,
                    escalation_stage,
                    containment_scope_name(boundary.scope()),
                    boundary.identity().owner_identifier(),
                )
                .with_identity(Some(worker_id), Some(&self.job_id), Some(&self.attempt_id))
                .with_containment(Some(boundary.root().value()), root_pid, None, [], 0)
                .with_timing(unix_time_millis(), 0, remaining),
            );
        }

        blockers.push(
            ShutdownBlocker::new(
                ShutdownBlockerKind::RegistryState,
                escalation_stage,
                "attempt_registry",
                join_state_name(self.join_state),
            )
            .with_identity(Some(worker_id), Some(&self.job_id), Some(&self.attempt_id))
            .with_timing(unix_time_millis(), 0, remaining),
        );
        blockers.truncate(MAX_SHUTDOWN_BLOCKERS);
        blockers
    }

    fn matches(&self, attempt_id: &str, generation: u64) -> bool {
        self.attempt_id == attempt_id && self.generation == generation
    }
}

#[derive(Default)]
struct RegistryState {
    shutdown: Option<WorkerShutdown>,
    jobs: BTreeMap<String, ActiveJobTask>,
    empty_waiters: Vec<Waker>,
}

/// Registry that owns every attempt from immediately before spawn until its
/// containment and resource-join proof has completed.
#[derive(Clone, Default)]
pub struct WorkerTaskRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl WorkerTaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an attempt before its future is spawned. Returns false after
    /// shutdown intake closes or when the job already has a local owner.
    pub fn register(&self, task: ActiveJobTask) -> bool {
        let mut state = self.lock();
        if state.shutdown.is_some() || state.jobs.contains_key(task.job_id()) {
            return false;
        }
        state.jobs.insert(task.job_id.clone(), task);
        true
    }

    pub fn active_jobs(&self) -> Vec<ActiveJobTask> {
        self.lock().jobs.values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().jobs.is_empty()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.lock().shutdown.is_some()
    }

    /// Creates a registry-owned notification for the current active set. New
    /// registration is normally closed with [`begin_shutdown`](Self::begin_shutdown)
    /// before callers wait on it.
    pub fn join_notification(&self) -> WorkerTaskJoinNotification {
        WorkerTaskJoinNotification {
            registry: self.clone(),
        }
    }

    /// Closes intake and atomically marks all current entries as shutdown-owned
    /// before publishing cancellation to their process owners.
    pub fn begin_shutdown(&self, shutdown: WorkerShutdown) -> WorkerTaskJoinNotification {
        let (shutdown, tasks) = {
            let mut state = self.lock();
            let shutdown = state.shutdown.map_or(shutdown, |old| old.max(shutdown));
            state.shutdown = Some(shutdown);
            let join_state = match shutdown {
                WorkerShutdown::Graceful => ActiveJobJoinState::CancellationRequested,
                WorkerShutdown::Crash => ActiveJobJoinState::HardKillRequested,
            };
            let tasks = state
                .jobs
                .values_mut()
                .map(|task| {
                    task.join_state = task.join_state.max(join_state);
                    task.clone()
                })
                .collect::<Vec<_>>();
            (shutdown, tasks)
        };
        for task in tasks {
            task.fence.close();
            match shutdown {
                WorkerShutdown::Graceful => task.cancellation.cancel(),
                WorkerShutdown::Crash => task.cancellation.hard_kill(),
            }
        }
        WorkerTaskJoinNotification {
            registry: self.clone(),
        }
    }

    /// Monotonically escalates every still-active attempt.
    pub fn request_all(&self, request: JobCancellationRequest) {
        let tasks = {
            let mut state = self.lock();
            let join_state = join_state_for_request(request);
            state
                .jobs
                .values_mut()
                .map(|task| {
                    task.join_state = task.join_state.max(join_state);
                    task.clone()
                })
                .collect::<Vec<_>>()
        };
        for task in tasks {
            task.fence.close();
            task.cancellation.request(request);
        }
    }

    pub(crate) fn mark_running(&self, task: &ActiveJobTask) {
        self.update(task, ActiveJobJoinState::Running);
    }

    pub(crate) fn mark_shutdown_blocker(
        &self,
        job_id: &str,
        attempt_id: &str,
        generation: u64,
        mut blocker: ShutdownBlocker,
    ) {
        let mut state = self.lock();
        let Some(task) = state
            .jobs
            .get_mut(job_id)
            .filter(|task| task.matches(attempt_id, generation))
        else {
            return;
        };
        task.join_state = task.join_state.max(ActiveJobJoinState::CleanupPending);
        if let Some(existing) = task
            .shutdown_blockers
            .iter_mut()
            .find(|existing| same_blocker(&existing.blocker, &blocker))
        {
            blocker.first_seen_millis = existing.blocker.first_seen_millis;
            existing.blocker = blocker.sanitized();
            return;
        }
        if task.shutdown_blockers.len() < MAX_SHUTDOWN_BLOCKERS.saturating_sub(1) {
            blocker.first_seen_millis = unix_time_millis();
            task.shutdown_blockers.push(ObservedShutdownBlocker {
                blocker: blocker.sanitized(),
                observed_at: Instant::now(),
            });
        }
    }

    pub(crate) fn mark_cleanup_pending(&self, job_id: &str, attempt_id: &str, generation: u64) {
        let mut state = self.lock();
        if let Some(task) = state
            .jobs
            .get_mut(job_id)
            .filter(|task| task.matches(attempt_id, generation))
        {
            task.join_state = task.join_state.max(ActiveJobJoinState::CleanupPending);
        }
    }

    pub(crate) fn task(
        &self,
        job_id: &str,
        attempt_id: &str,
        generation: u64,
    ) -> Option<ActiveJobTask> {
        self.lock()
            .jobs
            .get(job_id)
            .filter(|task| task.matches(attempt_id, generation))
            .cloned()
    }

    /// Exact-attempt cancellation linearization boundary. Identity lookup,
    /// fence closure, cooperative publication, and the monotonic join-state
    /// update happen under one registry lock, in that order.
    pub(crate) fn cancel_attempt(&self, job_id: &str, attempt_id: &str, generation: u64) -> bool {
        let mut state = self.lock();
        let Some(task) = state
            .jobs
            .get_mut(job_id)
            .filter(|task| task.matches(attempt_id, generation))
        else {
            return false;
        };
        task.fence.close();
        task.cancellation.cancel();
        task.join_state = task
            .join_state
            .max(ActiveJobJoinState::CancellationRequested);
        true
    }

    pub(crate) fn request_attempt(
        &self,
        job_id: &str,
        attempt_id: &str,
        generation: u64,
        request: JobCancellationRequest,
    ) -> bool {
        let mut state = self.lock();
        let Some(task) = state
            .jobs
            .get_mut(job_id)
            .filter(|task| task.matches(attempt_id, generation))
        else {
            return false;
        };
        task.fence.close();
        task.cancellation.request(request);
        task.join_state = task.join_state.max(join_state_for_request(request));
        true
    }

    /// Linearizes terminal publication with component shutdown, then removes
    /// the task and wakes registry-owned join notifications. The callback runs
    /// under the registry lock so shutdown cannot begin between the publication
    /// decision and the completion-queue send.
    pub(crate) fn finish_with(&self, task: &ActiveJobTask, publish: impl FnOnce(bool)) {
        let waiters = {
            let mut state = self.lock();
            let Some(current) = state
                .jobs
                .get_mut(task.job_id())
                .filter(|current| current.matches(task.attempt_id(), task.generation()))
            else {
                publish(false);
                return;
            };
            current.join_state = ActiveJobJoinState::Joined;
            publish(state.shutdown.is_none());
            state.jobs.remove(task.job_id());
            state
                .jobs
                .is_empty()
                .then(|| std::mem::take(&mut state.empty_waiters))
        };
        if let Some(waiters) = waiters {
            for waiter in waiters {
                waiter.wake();
            }
        }
    }

    /// Removes an entry after its owner has proven all local resources joined.
    pub fn mark_joined(&self, task: &ActiveJobTask) {
        self.finish_with(task, |_| {});
    }

    fn update(&self, task: &ActiveJobTask, join_state: ActiveJobJoinState) {
        let mut state = self.lock();
        if let Some(current) = state
            .jobs
            .get_mut(task.job_id())
            .filter(|current| current.matches(task.attempt_id(), task.generation()))
        {
            current.join_state = current.join_state.max(join_state);
        }
    }

    fn lock(&self) -> MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn join_state_for_request(request: JobCancellationRequest) -> ActiveJobJoinState {
    match request {
        JobCancellationRequest::Graceful => ActiveJobJoinState::CancellationRequested,
        JobCancellationRequest::ForcedTermination => ActiveJobJoinState::ForcedTerminationRequested,
        JobCancellationRequest::HardKill => ActiveJobJoinState::HardKillRequested,
    }
}

fn join_state_name(state: ActiveJobJoinState) -> &'static str {
    match state {
        ActiveJobJoinState::Registered => "registered",
        ActiveJobJoinState::Running => "running",
        ActiveJobJoinState::CancellationRequested => "cancellation_requested",
        ActiveJobJoinState::ForcedTerminationRequested => "forced_termination_requested",
        ActiveJobJoinState::HardKillRequested => "hard_kill_requested",
        ActiveJobJoinState::CleanupPending => "cleanup_pending",
        ActiveJobJoinState::Joined => "joined",
    }
}

fn containment_scope_name(scope: &temper_process_containment::ContainmentScope) -> &str {
    match scope {
        temper_process_containment::ContainmentScope::Job => "job",
        temper_process_containment::ContainmentScope::Tool => "tool",
        temper_process_containment::ContainmentScope::Agent => "agent",
        temper_process_containment::ContainmentScope::McpServer => "mcp_server",
        temper_process_containment::ContainmentScope::WorkerCommand => "worker_command",
        temper_process_containment::ContainmentScope::PrePush => "pre_push",
        temper_process_containment::ContainmentScope::Custom(name) => name,
    }
}

fn same_blocker(left: &ShutdownBlocker, right: &ShutdownBlocker) -> bool {
    left.kind == right.kind
        && left.owner_scope == right.owner_scope
        && left.owner_name == right.owner_name
        && left.owner_root == right.owner_root
        && left.root_pid == right.root_pid
        && left.trace_run_id == right.trace_run_id
        && left.trace_sequence == right.trace_sequence
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(duration_millis)
        .unwrap_or(0)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Registry-owned notification that completes only after every registered job
/// has supplied its local join proof and left the registry.
pub struct WorkerTaskJoinNotification {
    registry: WorkerTaskRegistry,
}

impl WorkerTaskJoinNotification {
    pub fn is_ready(&self) -> bool {
        self.registry.is_empty()
    }

    pub async fn wait(self) {
        std::future::poll_fn(|cx| {
            let mut state = self.registry.lock();
            if state.jobs.is_empty() {
                return Poll::Ready(());
            }
            if !state
                .empty_waiters
                .iter()
                .any(|waiter| waiter.will_wake(cx.waker()))
            {
                state.empty_waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await;
    }
}

#[cfg(test)]
#[path = "task_registry_tests.rs"]
mod tests;
