//! The agent-turn seam.
//!
//! [`CodingExecutor`](crate::coding_executor::CodingExecutor) owns the workspace
//! lifecycle — prepare the scoped checkout root, run one agent turn, map the
//! result to a
//! [`JobOutcome`](crate::executor::JobOutcome), commit/push or discard. The
//! *agent turn itself* is abstracted behind [`AgentRunner`] so the orchestration
//! is independent of how the turn is produced — and, crucially, so the worker
//! links **no** agent/LLM code: the agent runs out-of-process behind the
//! `smith-agent-protocol` wire contract.
//!
//! - [`OutOfProcessRunner`](crate::out_of_process_runner::OutOfProcessRunner)
//!   spawns an agent program (the `temper-agent` binary by default, or any
//!   coder) speaking the protocol: context in via `--context`, result back via
//!   `--result`.
//! - test fakes return scripted results without any subprocess.
//!
//! The agent has git credentials only via the prepared repo checkouts; it never
//! calls the forge API. The executor owns the final branch push.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use temper_protocol_activity::ModelFailureV1;
use temper_protocol_agent::{
    AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleEventV1, AgentLifecycleFrameV1,
    AgentLifecycleScopeV1, WorkspaceContext,
};
use temper_protocol_worker::{
    FailureClass, ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult,
};

use crate::executor::{AttemptFence, JobCancellation};
pub use temper_protocol_agent::WorkspaceResult;
use temper_worker_io::EngineTime;

mod submit_proof;
pub use submit_proof::{
    AcceptedSubmitProof, AcceptedSubmitProofStore, handle_submit_for_pr_with_proof,
};

/// Async worker-owned Forge reader. The exact job and attempt ids plus the
/// attempt's irreversible publication fence are supplied by the executor,
/// never by the model or child process.
pub type AgentForgeContextFuture =
    Pin<Box<dyn Future<Output = Result<ForgeContextResult, ForgeContextErrorCode>> + Send>>;
pub type AgentForgeContextHost = Arc<
    dyn Fn(String, String, AttemptFence, ForgeContextOperation) -> AgentForgeContextFuture
        + Send
        + Sync,
>;

const ATTEMPT_UNAVAILABLE_MESSAGE: &str = "agent attempt is no longer available";

/// An attempt-tagged lifecycle observation delivered at the worker boundary.
///
/// The reporter invocation is the receipt boundary: the worker-owned reporter
/// stamps its monotonic runtime clock rather than trusting a child clock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobProgress {
    pub attempt_id: String,
    /// Worker runtime-clock receipt stamp; child/source time is never trusted.
    pub received_at: EngineTime,
    pub frame: AgentLifecycleFrameV1,
}

/// Attempt-bound, typed progress delivery shared by real runners and fakes.
///
/// A reporter never accepts caller-selected worker/job identity. Old endpoints
/// retain their original attempt tag, and an optional worker-owned guard drops
/// reports after that attempt stops being current.
#[derive(Clone)]
pub struct JobProgressReporter {
    attempt_id: Arc<str>,
    next_seq: Arc<Mutex<u64>>,
    clock: Arc<dyn Fn() -> EngineTime + Send + Sync>,
    is_current: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    sink: Arc<dyn Fn(JobProgress) + Send + Sync>,
}

impl std::fmt::Debug for JobProgressReporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobProgressReporter")
            .field("attempt_id", &self.attempt_id)
            .finish_non_exhaustive()
    }
}

impl JobProgressReporter {
    /// Builds a reporter for one attempt. The callback should enqueue a worker
    /// completion; it must not perform blocking liveness policy work inline.
    pub fn new(
        attempt_id: impl Into<String>,
        sink: impl Fn(JobProgress) + Send + Sync + 'static,
    ) -> Self {
        Self::with_attempt_guard(attempt_id, |_| true, sink)
    }

    /// Builds a reporter with a worker-owned stale-attempt guard.
    pub fn with_attempt_guard(
        attempt_id: impl Into<String>,
        is_current: impl Fn(&str) -> bool + Send + Sync + 'static,
        sink: impl Fn(JobProgress) + Send + Sync + 'static,
    ) -> Self {
        Self::with_clock_and_attempt_guard(
            attempt_id,
            || EngineTime::from(temper_worker_io::engine_now()),
            is_current,
            sink,
        )
    }

    /// Builds a reporter with an explicit runtime clock. Simulations and pure
    /// tests use this seam to stamp deterministic receipt time.
    pub fn with_clock_and_attempt_guard(
        attempt_id: impl Into<String>,
        clock: impl Fn() -> EngineTime + Send + Sync + 'static,
        is_current: impl Fn(&str) -> bool + Send + Sync + 'static,
        sink: impl Fn(JobProgress) + Send + Sync + 'static,
    ) -> Self {
        Self {
            attempt_id: Arc::from(attempt_id.into()),
            next_seq: Arc::new(Mutex::new(1)),
            clock: Arc::new(clock),
            is_current: Arc::new(is_current),
            sink: Arc::new(sink),
        }
    }

    /// A reporter used by compatibility callers that do not yet supervise
    /// progress. It still validates and sequences typed fake reports.
    pub fn noop(attempt_id: impl Into<String>) -> Self {
        Self::new(attempt_id, |_| {})
    }

    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    /// Reports typed progress directly (the fake/in-process seam).
    pub fn report(&self, scope: AgentLifecycleScopeV1, event: AgentLifecycleEventV1) -> bool {
        // Serialize direct reporters so nested/concurrent fake scopes cannot
        // deliver a later sequence before an earlier one.
        let mut next_seq = self
            .next_seq
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seq = *next_seq;
        *next_seq = next_seq.saturating_add(1);
        self.accept_frame(AgentLifecycleFrameV1 {
            version: AGENT_LIFECYCLE_PROTOCOL_VERSION,
            seq,
            scope,
            event,
        })
    }

    /// Accepts an already-sequenced frame from an out-of-process endpoint.
    pub(crate) fn accept_frame(&self, frame: AgentLifecycleFrameV1) -> bool {
        if frame.validate().is_err() || !(self.is_current)(&self.attempt_id) {
            return false;
        }
        let progress = JobProgress {
            attempt_id: self.attempt_id.to_string(),
            received_at: (self.clock)(),
            frame,
        };
        // A broken observer cannot alter the assigned product run.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.sink)(progress))).is_ok()
    }
}

/// Complete input to one agent attempt. The context and checkout are borrowed;
/// the attempt identity and reporter are owned so runners may move them into an
/// async block safely.
#[derive(Clone)]
pub struct AgentRunRequest<'a> {
    pub job_id: &'a str,
    pub attempt_id: String,
    pub context: &'a WorkspaceContext,
    pub cwd: &'a Path,
    pub fence: AttemptFence,
    pub cancellation: JobCancellation,
    /// Explicit attempt-owned authority threaded into native and
    /// out-of-process containment composition.
    pub emergency_termination: temper_process_containment::EmergencyTerminationRegistry,
    pub progress: JobProgressReporter,
}

impl<'a> AgentRunRequest<'a> {
    pub fn new(
        job_id: &'a str,
        attempt_id: impl Into<String>,
        context: &'a WorkspaceContext,
        cwd: &'a Path,
        progress: JobProgressReporter,
    ) -> Self {
        Self::new_controlled(
            job_id,
            attempt_id,
            context,
            cwd,
            AttemptFence::open(),
            JobCancellation::default(),
            progress,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_controlled(
        job_id: &'a str,
        attempt_id: impl Into<String>,
        context: &'a WorkspaceContext,
        cwd: &'a Path,
        fence: AttemptFence,
        cancellation: JobCancellation,
        progress: JobProgressReporter,
    ) -> Self {
        let emergency_termination = cancellation.emergency_termination_registry();
        Self {
            job_id,
            attempt_id: attempt_id.into(),
            context,
            cwd,
            fence,
            cancellation,
            emergency_termination,
            progress,
        }
    }

    pub fn unsupervised(job_id: &'a str, context: &'a WorkspaceContext, cwd: &'a Path) -> Self {
        let attempt_id = job_id.to_string();
        let progress = JobProgressReporter::noop(attempt_id.clone());
        Self::new(job_id, attempt_id, context, cwd, progress)
    }
}

/// What a completed agent turn produced at the worker boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRunOutput {
    pub result: WorkspaceResult,
    /// Latest host-accepted submit proof, if any.
    pub accepted_submit: Option<AcceptedSubmitProof>,
    /// Bounded graph-result records from the first-party operator-local
    /// diagnostic transcript. Generic worker consumers ignore this field.
    pub operator_transcript: Vec<temper_protocol_activity::OperatorTranscriptToolResultV1>,
}

impl AgentRunOutput {
    pub fn new(result: WorkspaceResult) -> Self {
        Self {
            result,
            accepted_submit: None,
            operator_transcript: Vec::new(),
        }
    }

    pub fn with_accepted_submit(
        result: WorkspaceResult,
        accepted_submit: AcceptedSubmitProof,
    ) -> Self {
        Self {
            result,
            accepted_submit: Some(accepted_submit),
            operator_transcript: Vec::new(),
        }
    }
}

/// Why an agent turn could not produce a [`WorkspaceResult`].
///
/// Carries the [`FailureClass`] the executor reports to the daemon so the
/// classification (transient vs permanent vs protocol) lives with the runner
/// that knows the failure's nature, rather than being re-derived downstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRunError {
    pub class: FailureClass,
    pub message: String,
    /// Canonical model diagnostic retained independently of activity tracing.
    pub model_failure: Option<ModelFailureV1>,
}

impl AgentRunError {
    pub fn new(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            model_failure: None,
        }
    }

    /// Attaches and canonicalizes an authoritative first-party model failure.
    pub fn with_model_failure(mut self, mut model_failure: ModelFailureV1) -> Self {
        model_failure.normalize();
        self.model_failure = Some(model_failure);
        self
    }

    /// A transient failure: retrying later may succeed (provider/transport
    /// hiccup, subprocess spawn failure, ...).
    pub fn transient(message: impl Into<String>) -> Self {
        Self::new(FailureClass::Transient, message)
    }

    /// A permanent failure: the same input will fail the same way (the agent
    /// produced no usable product, an undeclared verdict, ...).
    pub fn permanent(message: impl Into<String>) -> Self {
        Self::new(FailureClass::Permanent, message)
    }
}

impl std::fmt::Display for AgentRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentRunError {}

/// Runs one role-aware coding/triage/review turn in a prepared workspace root.
///
/// `context` is the work-item context the worker assembled (repositories, role,
/// branch, verdict vocabulary, ...). `cwd` is the prepared scoped workspace root
/// the turn operates on: writable repos live in sibling dirs where the agent
/// leaves product diffs; read-only repos are inspected for verdicts. The runner
/// must not commit, push, or otherwise mutate Forge state — the executor owns
/// that.
pub trait AgentRunner: Send + Sync {
    fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> impl std::future::Future<Output = Result<AgentRunOutput, AgentRunError>> + Send;

    /// Attempt-aware run seam used by [`CodingExecutor`](crate::CodingExecutor).
    /// Legacy runners remain source-compatible and receive the same context;
    /// lifecycle-capable runners override this method to use `request.progress`.
    fn run_request(
        &self,
        request: AgentRunRequest<'_>,
    ) -> impl std::future::Future<Output = Result<AgentRunOutput, AgentRunError>> + Send {
        self.run(request.job_id, request.context, request.cwd)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};

    use temper_protocol_agent::{
        PROTOCOL_VERSION, SubmitForPrRequest, SubmitForPrResponse, WorkspaceContext,
        WorkspaceWorkItem,
    };

    use crate::pre_push::WorkspaceFingerprint;

    use super::*;

    #[test]
    fn submit_completion_after_fence_is_rejected_and_proof_is_cleared() {
        let store = AcceptedSubmitProofStore::new();
        *store.inner.lock().expect("proof store") = Some(AcceptedSubmitProof {
            response: SubmitForPrResponse::accepted("old proof"),
            fingerprint: WorkspaceFingerprint { repos: Vec::new() },
        });
        let fence = AttemptFence::open();
        let cancellation = JobCancellation::default();
        let released = Arc::new(AtomicBool::new(false));
        let released_for_gate = Arc::clone(&released);
        let request = SubmitForPrRequest {
            protocol_version: PROTOCOL_VERSION,
            correlation_key: "attempt-fence-test".to_string(),
            role: "engineer".to_string(),
            action: "open_pr".to_string(),
            summary: None,
        };
        let context = WorkspaceContext {
            trace_context: None,
            artifact_context: None,
            repos: Vec::new(),
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code_ready".to_string(),
                kind: "code".to_string(),
                target: "Issue { number: 605 }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "attempt-fence-test".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: None,
        };
        let temporary = tempfile::tempdir().expect("submit test workspace");
        let mut call = Box::pin(handle_submit_for_pr_with_proof(
            &store,
            &fence,
            &cancellation,
            move |_request, _context, _cwd| {
                std::future::poll_fn(move |_cx| {
                    released_for_gate
                        .load(Ordering::Acquire)
                        .then(|| SubmitForPrResponse::accepted("late acceptance"))
                        .map_or(Poll::Pending, Poll::Ready)
                })
            },
            request,
            context,
            temporary.path().to_path_buf(),
        ));
        let mut task_context = Context::from_waker(Waker::noop());
        assert!(call.as_mut().poll(&mut task_context).is_pending());

        fence.close();
        released.store(true, Ordering::Release);
        let Poll::Ready(response) = call.as_mut().poll(&mut task_context) else {
            panic!("released submit gate did not settle")
        };
        assert!(!response.accepted);
        assert_eq!(response.message, ATTEMPT_UNAVAILABLE_MESSAGE);
        assert_eq!(store.latest(), None);
    }

    #[test]
    fn direct_progress_is_attempt_bound_sequenced_and_runtime_stamped() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_for_sink = Arc::clone(&observed);
        let reporter = JobProgressReporter::with_clock_and_attempt_guard(
            "attempt-1",
            || EngineTime::from_nanos(42),
            |attempt| attempt == "attempt-1",
            move |progress| observed_for_sink.lock().unwrap().push(progress),
        );
        let scope = AgentLifecycleScopeV1 {
            id: "main".to_string(),
            parent_id: None,
        };
        assert!(reporter.report(scope.clone(), AgentLifecycleEventV1::SteeringApplied));
        assert!(reporter.report(
            scope,
            AgentLifecycleEventV1::AgentFinished {
                status: temper_protocol_agent::AgentLifecycleAgentStatusV1::Succeeded,
            }
        ));

        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].attempt_id, "attempt-1");
        assert_eq!(observed[0].received_at, EngineTime::from_nanos(42));
        assert_eq!(observed[0].frame.seq, 1);
        assert_eq!(observed[1].frame.seq, 2);
    }
}
