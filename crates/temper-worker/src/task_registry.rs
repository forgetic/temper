//! Explicit ownership and shutdown coordination for worker-spawned tasks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};

use crate::executor::{AttemptFence, JobCancellation, JobCancellationRequest};

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

/// Exact registry entry that remained unresolved at the shutdown deadline.
#[derive(Clone, Debug)]
pub struct WorkerShutdownBlocker {
    pub identity: WorkerAttemptIdentity,
    pub registry_state: ActiveJobJoinState,
    pub emergency_termination: temper_process_containment::EmergencyTerminationSnapshot,
}

/// Result of a bounded worker join. Unresolved entries remain registered and
/// fenced; this report is evidence only and never fabricates local quiescence.
#[derive(Clone, Debug, Default)]
pub struct WorkerShutdownReport {
    pub joined_attempts: Vec<WorkerAttemptIdentity>,
    pub unresolved_blockers: Vec<WorkerShutdownBlocker>,
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

#[derive(Default)]
struct ComponentTaskState {
    accepting: bool,
    active: usize,
    waiters: Vec<Waker>,
}

/// Join accounting for transport/outbox tasks that are not attempt owners.
#[derive(Clone)]
pub(crate) struct WorkerComponentTasks {
    state: Arc<Mutex<ComponentTaskState>>,
}

impl Default for WorkerComponentTasks {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(ComponentTaskState {
                accepting: true,
                ..ComponentTaskState::default()
            })),
        }
    }
}

impl WorkerComponentTasks {
    pub(crate) fn register(&self) -> Option<WorkerComponentTaskGuard> {
        let mut state = self.lock();
        if !state.accepting {
            return None;
        }
        state.active += 1;
        Some(WorkerComponentTaskGuard {
            tasks: self.clone(),
        })
    }

    pub(crate) fn stop_accepting(&self) {
        self.lock().accepting = false;
    }

    pub(crate) async fn wait_empty(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.lock();
            if state.active == 0 {
                return Poll::Ready(());
            }
            if !state
                .waiters
                .iter()
                .any(|waiter| waiter.will_wake(cx.waker()))
            {
                state.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await;
    }

    fn lock(&self) -> MutexGuard<'_, ComponentTaskState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct WorkerComponentTaskGuard {
    tasks: WorkerComponentTasks,
}

impl Drop for WorkerComponentTaskGuard {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self.tasks.lock();
            state.active = state.active.saturating_sub(1);
            (state.active == 0).then(|| std::mem::take(&mut state.waiters))
        };
        if let Some(waiters) = waiters {
            for waiter in waiters {
                waiter.wake();
            }
        }
    }
}

#[cfg(test)]
#[path = "task_registry_tests.rs"]
mod tests;
