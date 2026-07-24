// SPDX-License-Identifier: MPL-2.0

//! Caller-controlled executor and progress source for the real-worker lab.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use temper_protocol_worker::Assign;
use temper_worker::{
    AgentLifecycleEventV1, AgentLifecycleScopeV1, JobExecutionContext, JobExecutor, JobOutcome,
    JobProgressReporter, StubExecutor,
};

#[derive(Default)]
struct ForgeGateState {
    resolved: bool,
    waker: Option<Waker>,
}

#[derive(Clone, Default)]
struct ForgeGate(Arc<Mutex<ForgeGateState>>);

impl ForgeGate {
    fn wait(&self) -> ControlledForgeFuture {
        ControlledForgeFuture(self.clone())
    }

    fn resolve(&self) {
        let waker = {
            let mut state = self.0.lock().expect("controlled Forge gate lock");
            state.resolved = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct ControlledForgeFuture(ForgeGate);

impl Future for ControlledForgeFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.0.lock().expect("controlled Forge gate lock");
        if state.resolved {
            Poll::Ready(())
        } else {
            if !state
                .waker
                .as_ref()
                .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            Poll::Pending
        }
    }
}

impl Drop for ControlledForgeFuture {
    fn drop(&mut self) {
        self.0
            .0
            .lock()
            .expect("controlled Forge gate lock")
            .waker
            .take();
    }
}

/// Observable state from [`ControllableExecutor`]. Ordered vectors make the
/// exact schedule useful in assertion failures and deterministic across seeds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControllableExecutorState {
    pub starts: Vec<String>,
    pub completions: Vec<String>,
    pub cancellations: Vec<String>,
    pub forge_future_resolutions: Vec<String>,
    pub late_progress_attempts: Vec<String>,
    pub accepted_late_progress: Vec<String>,
    /// Downstream executor stages that would be authoritative only if the
    /// controlled Forge future completed while its attempt fence was open.
    pub result_file_acceptances: Vec<String>,
    pub validations: Vec<String>,
    pub pushes: Vec<String>,
    pub forge_mutations: Vec<String>,
}

#[derive(Default)]
struct ControllableExecutorInner {
    state: Mutex<ControllableExecutorState>,
    gates: Mutex<BTreeMap<String, ForgeGate>>,
    reporters: Mutex<BTreeMap<String, JobProgressReporter>>,
}

/// A deterministic executor/progress source for high-fidelity worker tests.
/// Selected jobs report a typed `forge_list_related` start boundary and then
/// park on a caller-resolvable future. Every other job uses the production
/// success stub. The shell still owns cancellation and the attempt fence.
#[derive(Clone, Default)]
pub struct ControllableExecutor(Arc<ControllableExecutorInner>);

impl ControllableExecutor {
    pub fn with_hung_forge_job(job_id: impl Into<String>) -> Self {
        let executor = Self::default();
        executor
            .0
            .gates
            .lock()
            .expect("controlled Forge gates lock")
            .insert(job_id.into(), ForgeGate::default());
        executor
    }

    pub fn snapshot(&self) -> ControllableExecutorState {
        self.0
            .state
            .lock()
            .expect("controllable executor state lock")
            .clone()
    }

    /// Resolve the selected Forge future. After watchdog cancellation this
    /// deliberately has no continuation to wake because the executor future
    /// has already been dropped and joined.
    pub fn resolve_forge_future(&self, job_id: &str) -> bool {
        let gate = self
            .0
            .gates
            .lock()
            .expect("controlled Forge gates lock")
            .get(job_id)
            .cloned();
        let Some(gate) = gate else {
            return false;
        };
        self.record(|state| state.forge_future_resolutions.push(job_id.to_string()));
        gate.resolve();
        true
    }

    /// Exercise the old progress source after cancellation. The worker-owned
    /// attempt guard must reject the frame before it reaches the machine.
    pub fn report_late_progress(&self, job_id: &str) -> bool {
        let reporter = self
            .0
            .reporters
            .lock()
            .expect("controllable executor reporters lock")
            .get(job_id)
            .cloned();
        let Some(reporter) = reporter else {
            return false;
        };
        let accepted = reporter.report(
            AgentLifecycleScopeV1 {
                id: "main".to_string(),
                parent_id: None,
            },
            AgentLifecycleEventV1::SteeringApplied,
        );
        self.record(|state| {
            state.late_progress_attempts.push(job_id.to_string());
            if accepted {
                state.accepted_late_progress.push(job_id.to_string());
            }
        });
        accepted
    }

    fn record(&self, update: impl FnOnce(&mut ControllableExecutorState)) {
        update(
            &mut self
                .0
                .state
                .lock()
                .expect("controllable executor state lock"),
        );
    }
}

struct ExecutionDropGuard {
    executor: ControllableExecutor,
    job_id: String,
    completed: bool,
}

impl Drop for ExecutionDropGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.executor
                .record(|state| state.cancellations.push(self.job_id.clone()));
        }
    }
}

impl JobExecutor for ControllableExecutor {
    fn execute(
        &self,
        assign: Assign,
        context: JobExecutionContext,
    ) -> impl Future<Output = JobOutcome> + Send {
        let executor = self.clone();
        async move {
            let job_id = assign.job_id.clone();
            executor.record(|state| state.starts.push(job_id.clone()));
            let gate = executor
                .0
                .gates
                .lock()
                .expect("controlled Forge gates lock")
                .get(&job_id)
                .cloned();
            if let Some(gate) = gate {
                executor
                    .0
                    .reporters
                    .lock()
                    .expect("controllable executor reporters lock")
                    .insert(job_id.clone(), context.progress.clone());
                assert!(context.progress.report(
                    AgentLifecycleScopeV1 {
                        id: "main".to_string(),
                        parent_id: None,
                    },
                    AgentLifecycleEventV1::ToolStarted {
                        call_id: "forge-future-1".to_string(),
                        name: "forge_list_related".to_string(),
                    },
                ));
                let mut guard = ExecutionDropGuard {
                    executor: executor.clone(),
                    job_id: job_id.clone(),
                    completed: false,
                };
                let completed = context.cancellation.run(gate.wait()).await;
                if completed.is_none() {
                    return JobOutcome::Failure {
                        class: temper_protocol_worker::FailureClass::Canceled,
                        message: "controlled Forge future cancelled".to_string(),
                        model_failure: None,
                        session_recovery: None,
                    };
                }
                executor.record(|state| {
                    state.result_file_acceptances.push(job_id.clone());
                    state.validations.push(job_id.clone());
                    state.pushes.push(job_id.clone());
                    state.forge_mutations.push(job_id.clone());
                    state.completions.push(job_id);
                });
                guard.completed = true;
            } else {
                executor.record(|state| state.completions.push(job_id));
            }
            <StubExecutor as JobExecutor>::execute(&StubExecutor::success(), assign, context).await
        }
    }
}
