//! The orchestration **worker**: long-polls the daemon for jobs and drives one
//! agent turn per job out-of-process behind the agent protocol.
//!
//! ## Config objects
//!
//! The worker subsystem is configured by one struct, [`WorkerConfig`], passed to
//! the [`run_worker`] factory. It is constructible **in memory** (no files, env,
//! or args) so unit and integration tests can stand a worker up directly. It
//! carries the worker's identity/cadence knobs **and** the per-role git
//! identities ([`WorkerConfig::role_identities`]) — the single source of truth
//! the coding executor sources its identities from, rather than threading them
//! in separately. The adapter that builds the config unwraps the engine-side
//! `SecretString` push tokens at that boundary. Per the per-subsystem
//! config-object rule, factories with long parameter lists take a config object;
//! small ones stay as-is.

pub mod agent_runner;
pub mod agent_session;
pub mod client;
pub mod coding_executor;
pub mod config;
pub mod context_client;
pub mod executor;
mod lifecycle_hook;
mod managed_effect;
pub mod observability;
pub mod out_of_process_runner;
pub mod pr_freshness;
pub mod pre_push;
pub mod result_outbox;
pub mod run;
pub mod trace;
pub mod transport;
pub mod worker_machine;
pub mod worker_shell;
pub mod workspace;

pub use agent_runner::{
    AcceptedSubmitProof, AcceptedSubmitProofStore, AgentForgeContextFuture, AgentForgeContextHost,
    AgentRunError, AgentRunOutput, AgentRunRequest, AgentRunner, JobProgress, JobProgressReporter,
    WorkspaceResult, handle_submit_for_pr_with_proof,
};
pub use agent_session::{AgentSessionStore, AgentSessionStoreError};
pub use coding_executor::{CodingExecutor, CodingExecutorConfig};
pub use config::{
    AgentProviderChoice, AgentSurface, AnvilNativeAgentSurface, CapabilitySpec, CodingSurface,
    ExecutorSelection, ParseOutcome, USAGE, WorkerAgentTraceConfig, WorkerConfig,
    WorkerLivenessLimits, WorkerParams, parse, prepare_result_root, role_identities_from_env,
};
pub use context_client::{
    ContextClientError, ForgeContextClient, HttpForgeContextClient, forge_context_host,
};
pub use executor::{
    AttemptFence, CancellationOutcome, DescendantCleanupStatus, JobAttempt, JobCancellation,
    JobCancellationRequest, JobCleanup, JobExecutionContext, JobExecutor, JobOutcome, StubExecutor,
    job_result, job_result_for_attempt,
};
pub use lifecycle_hook::{WorkerLifecycleCheckpoint, WorkerLifecycleHook};
pub use observability::{assigned_job_line, registered_worker_line, result_sent_line};
pub use out_of_process_runner::{JobQuiesced, OutOfProcessRunner};
pub use pr_freshness::{
    HttpPrFreshnessGuard, PrFreshnessFailure, PrFreshnessGuard,
    map_response as map_pr_freshness_response,
};
pub use pre_push::{
    PrePushCommandResult, PrePushError, PrePushReport, PrePushStatus, WorkspaceFingerprint,
    WorkspaceFingerprintError, final_pre_push_response, fingerprint_writable_repos,
    fingerprint_writable_repos_blocking, run_pre_push_checks, submit_for_pr_pre_push_response,
};
pub use result_outbox::{
    RESULT_OUTBOX_VERSION, ResultAcknowledgement, ResultAssignmentIdentity, ResultDeliveryState,
    ResultOutbox, ResultOutboxEntry, ResultOutboxError,
};
pub use run::{
    WorkerComponentHandle, run_worker, run_worker_with_transport, start_worker_with_transport,
    start_worker_with_transport_and_hook,
};
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub use temper_process_containment::dispatch_linux_supervisor_helper;
pub use temper_protocol_agent::{
    AGENT_LIFECYCLE_ADDRESS_FLAG, AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentLifecycleAgentStatusV1,
    AgentLifecycleCancellationAckV1, AgentLifecycleCancellationAcknowledgementV1,
    AgentLifecycleCommandV1, AgentLifecycleEventV1, AgentLifecycleFrameV1, AgentLifecycleHelloV1,
    AgentLifecycleModelStatusV1, AgentLifecycleScopeV1, AgentLifecycleToolStatusV1,
    AgentRuntimeLimitsV1, AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode,
    CodebaseMemoryToolConfig, RUNTIME_LIMITS_FLAG,
};
pub use temper_protocol_worker::WorkerAuth;
pub use trace::{
    ActivityEndpoint, MAX_CHILD_ACTIVITY_FRAME_BYTES, MAX_CHILD_ACTIVITY_RECORD_BYTES,
    RecoveredTraceRun, TraceCollector, TraceError, TraceManifestV1, TraceRun,
    WORKER_SPOOL_RUN_CAPACITY,
};
pub use transport::{HttpTransport, Transport};
pub use worker_machine::{
    ActiveOperation, CancellationStatus, JobPhase, JobWatchState, OperationId, OperationKind,
    ResultDeliveryStatus, ResultDurabilityStatus, TimeoutReason, TimeoutState, WatchdogTimerKind,
    WorkerCompletion, WorkerMachine, WorkerRequest,
};
pub use workspace::{
    PreparationOutcome, QuarantineManifest, RecoveryContext, RoleGitIdentity,
    ScopedWorkspaceCleanupError, ScopedWorkspaceCleanupOutcome, ScopedWorkspacePathError,
    Workspace, WorkspaceConfig, WorkspaceError, cleanup_scoped_workspace,
    cleanup_scoped_workspace_sync, forgejo_remote_url, scoped_workspace_root,
    workspace_scope_component,
};
