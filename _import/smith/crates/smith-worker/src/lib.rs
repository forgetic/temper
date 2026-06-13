pub mod agent_runner;
pub mod client;
pub mod coding_executor;
pub mod config;
pub mod executor;
pub mod observability;
pub mod out_of_process_runner;
pub mod progress_relay;
pub mod run;
pub mod worker_machine;
pub mod worker_shell;
pub mod workspace;

pub use agent_runner::{
    AgentRunError, AgentRunner, LoggingProgressSink, NullProgressSink, ProgressSink,
    WorkspaceResult,
};
pub use coding_executor::{CodingExecutor, CodingExecutorConfig};
pub use config::{
    AgentAuthChoice, AgentSurface, AnvilNativeAgentSurface, CapabilitySpec, CodingSurface,
    ExecutorSelection, ParseOutcome, USAGE, WorkerConfig, WorkerParams, parse,
    role_identities_from_env,
};
pub use executor::{JobExecutor, JobOutcome, StubExecutor, job_result};
pub use observability::{assigned_job_line, registered_worker_line, result_sent_line};
pub use out_of_process_runner::OutOfProcessRunner;
pub use progress_relay::{DaemonRelayProgressSink, progress_message};
pub use run::run_worker;
pub use worker_machine::{WorkerCompletion, WorkerMachine, WorkerRequest};
pub use workspace::{
    RoleGitIdentity, Workspace, WorkspaceConfig, WorkspaceError, forgejo_remote_url,
};
