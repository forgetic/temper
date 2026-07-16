//! Per-run ownership, cancellation, and quiescence for model/tool tasks.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use futures::future::Either;
use temper_agent_io::CqSender;

use crate::machine::{AgentCompletion, BatchGeneration, OperationGeneration};

/// A small cancellation primitive whose future wakes immediately when the
/// shell cancels a run. It intentionally uses no wall clock and can therefore
/// participate in the same injected-runtime scheduling as operation deadlines.
#[derive(Clone, Default)]
pub(super) struct CancellationToken {
    inner: Arc<Mutex<CancellationState>>,
}

#[derive(Default)]
struct CancellationState {
    cancelled: bool,
    wakers: Vec<Waker>,
}

impl CancellationToken {
    pub(super) fn cancel(&self) {
        let wakers = {
            let mut state = self.inner.lock().expect("cancellation lock");
            if state.cancelled {
                return;
            }
            state.cancelled = true;
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub(super) async fn cancelled(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.inner.lock().expect("cancellation lock");
            if state.cancelled {
                Poll::Ready(())
            } else {
                if !state.wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
                    state.wakers.push(cx.waker().clone());
                }
                Poll::Pending
            }
        })
        .await
    }
}

pub(super) async fn cancel_or<F: Future>(
    cancellation: &CancellationToken,
    future: F,
) -> Option<F::Output> {
    match futures::future::select(Box::pin(cancellation.cancelled()), Box::pin(future)).await {
        Either::Left(_) => None,
        Either::Right((output, _)) => Some(output),
    }
}

#[derive(Clone)]
pub(crate) struct RunTaskGroup {
    inner: Arc<Mutex<TaskGroupState>>,
    cq: CqSender<AgentCompletion>,
}

#[derive(Default)]
struct TaskGroupState {
    next_id: u64,
    tasks: BTreeMap<u64, CancellationToken>,
    cancellation_generation: Option<(OperationGeneration, BatchGeneration)>,
    quiescence_waiters: Vec<Waker>,
}

pub(super) struct ActiveTaskGuard {
    group: RunTaskGroup,
    id: u64,
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        self.group.task_finished(self.id);
    }
}

/// Cancels tasks if the run future is dropped by its caller. Normal and
/// machine-driven cancellation disarm this guard only after joining the group.
pub(crate) struct RunDropGuard {
    group: RunTaskGroup,
    armed: bool,
}

impl RunDropGuard {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunDropGuard {
    fn drop(&mut self) {
        if self.armed {
            self.group.cancel_without_completion();
        }
    }
}

impl RunTaskGroup {
    pub(super) fn new(cq: CqSender<AgentCompletion>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TaskGroupState::default())),
            cq,
        }
    }

    pub(super) fn register(&self) -> (CancellationToken, ActiveTaskGuard) {
        let (id, cancellation) = {
            let mut state = self.inner.lock().expect("task group lock");
            let id = state.next_id;
            state.next_id = state
                .next_id
                .checked_add(1)
                .expect("agent task-group identity exhausted");
            let cancellation = CancellationToken::default();
            state.tasks.insert(id, cancellation.clone());
            (id, cancellation)
        };
        (
            cancellation,
            ActiveTaskGuard {
                group: self.clone(),
                id,
            },
        )
    }

    pub(super) fn cancel_all(
        &self,
        operation_generation: OperationGeneration,
        batch_generation: BatchGeneration,
    ) {
        let (tokens, quiesced) = {
            let mut state = self.inner.lock().expect("task group lock");
            state.cancellation_generation = Some((operation_generation, batch_generation));
            let tokens = state.tasks.values().cloned().collect::<Vec<_>>();
            let quiesced = if tokens.is_empty() {
                state.cancellation_generation.take()
            } else {
                None
            };
            (tokens, quiesced)
        };
        for token in tokens {
            token.cancel();
        }
        if let Some((operation_generation, batch_generation)) = quiesced {
            let _ = self.cq.send(AgentCompletion::TasksQuiesced {
                operation_generation,
                batch_generation,
            });
        }
    }

    fn cancel_without_completion(&self) {
        let tokens = {
            let mut state = self.inner.lock().expect("task group lock");
            state.cancellation_generation = None;
            state.tasks.values().cloned().collect::<Vec<_>>()
        };
        for token in tokens {
            token.cancel();
        }
    }

    fn task_finished(&self, id: u64) {
        let (quiesced, waiters) = {
            let mut state = self.inner.lock().expect("task group lock");
            state.tasks.remove(&id);
            if state.tasks.is_empty() {
                (
                    state.cancellation_generation.take(),
                    std::mem::take(&mut state.quiescence_waiters),
                )
            } else {
                (None, Vec::new())
            }
        };
        for waker in waiters {
            waker.wake();
        }
        if let Some((operation_generation, batch_generation)) = quiesced {
            let _ = self.cq.send(AgentCompletion::TasksQuiesced {
                operation_generation,
                batch_generation,
            });
        }
    }

    pub(crate) async fn wait_for_quiescence(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.inner.lock().expect("task group lock");
            if state.tasks.is_empty() {
                Poll::Ready(())
            } else {
                if !state
                    .quiescence_waiters
                    .iter()
                    .any(|waker| waker.will_wake(cx.waker()))
                {
                    state.quiescence_waiters.push(cx.waker().clone());
                }
                Poll::Pending
            }
        })
        .await
    }

    pub(crate) fn drop_guard(&self) -> RunDropGuard {
        RunDropGuard {
            group: self.clone(),
            armed: true,
        }
    }
}
