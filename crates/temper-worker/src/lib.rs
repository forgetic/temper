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
pub mod observability;
pub mod out_of_process_runner;
pub mod pr_freshness;
pub mod pre_push;
pub mod run;
pub mod trace;
pub mod transport;
pub mod worker_machine;
pub mod worker_shell;
pub mod workspace;

pub use agent_runner::{
    AcceptedSubmitProof, AcceptedSubmitProofStore, AgentForgeContextFuture, AgentForgeContextHost,
    AgentRunError, AgentRunOutput, AgentRunner, WorkspaceResult, handle_submit_for_pr_with_proof,
};
pub use agent_session::{AgentSessionStore, AgentSessionStoreError};
pub use coding_executor::{CodingExecutor, CodingExecutorConfig};
pub use config::{
    AgentProviderChoice, AgentSurface, AnvilNativeAgentSurface, CapabilitySpec, CodingSurface,
    ExecutorSelection, ParseOutcome, USAGE, WorkerAgentTraceConfig, WorkerConfig, WorkerParams,
    parse, role_identities_from_env,
};
pub use context_client::{
    ContextClientError, ForgeContextClient, HttpForgeContextClient, forge_context_host,
};
pub use executor::{JobExecutor, JobOutcome, StubExecutor, job_result};
pub use observability::{assigned_job_line, registered_worker_line, result_sent_line};
pub use out_of_process_runner::OutOfProcessRunner;
pub use pr_freshness::{
    HttpPrFreshnessGuard, PrFreshnessFailure, PrFreshnessGuard,
    map_response as map_pr_freshness_response,
};
pub use pre_push::{
    PrePushCommandResult, PrePushError, PrePushReport, PrePushStatus, WorkspaceFingerprint,
    WorkspaceFingerprintError, final_pre_push_response, fingerprint_writable_repos,
    fingerprint_writable_repos_blocking, run_pre_push_checks, submit_for_pr_pre_push_response,
    submit_for_pr_pre_push_response_blocking,
};
pub use run::{
    WorkerComponentHandle, run_worker, run_worker_with_transport, start_worker_with_transport,
};
pub use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
};
pub use temper_protocol_worker::WorkerAuth;
pub use trace::{
    ActivityEndpoint, MAX_CHILD_ACTIVITY_FRAME_BYTES, RecoveredTraceRun, TraceCollector,
    TraceError, TraceManifestV1, TraceRun, WORKER_SPOOL_RUN_CAPACITY,
};
pub use transport::{HttpTransport, Transport};
pub use worker_machine::{WorkerCompletion, WorkerMachine, WorkerRequest};
pub use workspace::{
    PreparationOutcome, QuarantineManifest, RecoveryContext, RoleGitIdentity,
    ScopedWorkspaceCleanupError, ScopedWorkspaceCleanupOutcome, ScopedWorkspacePathError,
    Workspace, WorkspaceConfig, WorkspaceError, cleanup_scoped_workspace,
    cleanup_scoped_workspace_sync, forgejo_remote_url, scoped_workspace_root,
    workspace_scope_component,
};
