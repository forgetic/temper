//! Join accounting and typed diagnostics for worker component tasks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};
use std::time::Instant;

use temper_protocol_worker::{
    MAX_SHUTDOWN_BLOCKERS, ShutdownBlocker, ShutdownBlockerKind, ShutdownEscalationStage,
};

use super::{duration_millis, unix_time_millis};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WorkerComponentTaskKind {
    ResultDelivery,
    ResultRecordingAcknowledgement,
    Transport,
    BackgroundComponent,
}

impl WorkerComponentTaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResultDelivery => "result_delivery",
            Self::ResultRecordingAcknowledgement => "result_recording_acknowledgement",
            Self::Transport => "transport",
            Self::BackgroundComponent => "background_component",
        }
    }
}

#[derive(Default)]
struct ComponentTaskState {
    accepting: bool,
    active: BTreeMap<WorkerComponentTaskKind, usize>,
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
    pub(crate) fn register(
        &self,
        kind: WorkerComponentTaskKind,
    ) -> Option<WorkerComponentTaskGuard> {
        let mut state = self.lock();
        if !state.accepting {
            return None;
        }
        *state.active.entry(kind).or_default() += 1;
        Some(WorkerComponentTaskGuard {
            tasks: self.clone(),
            kind,
        })
    }

    pub(crate) fn stop_accepting(&self) {
        self.lock().accepting = false;
    }

    pub(crate) fn shutdown_blockers(
        &self,
        worker_id: &str,
        escalation_stage: ShutdownEscalationStage,
        deadline: Instant,
    ) -> Vec<ShutdownBlocker> {
        let remaining = duration_millis(deadline.saturating_duration_since(Instant::now()));
        self.lock()
            .active
            .iter()
            .take(MAX_SHUTDOWN_BLOCKERS)
            .map(|(kind, count)| {
                let blocker_kind = if *kind == WorkerComponentTaskKind::ResultDelivery {
                    ShutdownBlockerKind::ResultDelivery
                } else {
                    ShutdownBlockerKind::ComponentTask
                };
                let mut blocker = ShutdownBlocker::new(
                    blocker_kind,
                    escalation_stage,
                    "worker_component",
                    kind.as_str(),
                )
                .with_identity(Some(worker_id), None, None)
                .with_timing(unix_time_millis(), 0, remaining);
                blocker.occurrences = u64::try_from(*count).unwrap_or(u64::MAX);
                blocker
            })
            .collect()
    }

    pub(crate) async fn wait_empty(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.lock();
            if state.active.is_empty() {
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
    kind: WorkerComponentTaskKind,
}

impl Drop for WorkerComponentTaskGuard {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self.tasks.lock();
            if let Some(active) = state.active.get_mut(&self.kind) {
                *active = active.saturating_sub(1);
                if *active == 0 {
                    state.active.remove(&self.kind);
                }
            }
            state
                .active
                .is_empty()
                .then(|| std::mem::take(&mut state.waiters))
        };
        if let Some(waiters) = waiters {
            for waiter in waiters {
                waiter.wake();
            }
        }
    }
}
