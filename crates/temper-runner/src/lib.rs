//! Reusable backend-agnostic runner primitives for Temper.
//!
//! `temper-runner` contains production orchestration building blocks that sit
//! above `temper-workflow` and coordinate only through the portable
//! [`temper_forge::Forge`] interface. It intentionally contains no concrete
//! provider adapters and no agent/provider behavior.
//!
//! Three constraints guide this crate:
//!
//! 1. **Primitives are the deliverable.** Orchestration-shaped logic belongs in
//!    reusable runner primitives; fakes and real adapters plug in at the edges.
//! 2. **Forward-compatible topology.** The same worker logic must run composed
//!    in one process or split one role per process, coordinating only through
//!    the Forge.
//! 3. **Layered testing.** Narrow pure/backend tests should exercise reusable
//!    primitives before broader end-to-end scenarios reuse the same code.
//!
//! The fake/real boundary is explicit: this crate owns composition, drivers,
//! role-scoped tools, and worker scheduling; tests may plug in fake agents,
//! fake outside-world producers, or memory identity handles, while production
//! plugs in real agents, authenticated Forge handles, and poll/webhook drivers.
//! CI is deliberately asymmetric: the workflow engine reads native CI jobs from
//! the Forge, while tests can add a fake producer through [`CiSink`] and
//! [`CiWorker`]; production has no `CiSink` because the provider's CI system is
//! the producer.
//!
//! The crate now provides:
//!
//! - [`RunnerConfig`], which keeps repository, identity, PR-create, external-tool
//!   binding, lease, and polling settings independent of process topology.
//! - [`scan`], which reads fresh Forge state through queue-derived candidate
//!   queries and turns active queue members into role-addressed [`WorkItem`]s
//!   without mutating anything; [`scan_automated_queues`] uses the same bounded
//!   candidate path for mechanically serviced queues.
//! - [`Agent`] and [`RoleTools`], which define the production tool boundary:
//!   agents mutate workflow state only by running authorized transitions or the
//!   idempotent pull-request creation seam through role-scoped tools. The
//!   workflow-role decision process adapter is provider-neutral and still runs
//!   chosen actions only through this boundary.
//! - [`Worker`], [`RoleWorker`], and [`MechanicalWorker`]: role workers re-scan
//!   judgment queues and delegate to agents, while the mechanical worker runs
//!   reconcile → apply and then declared queue automation once per normal tick
//!   without spawning agents.
//! - [`RepositorySet`], [`MultiRepoRoleWorker`], and
//!   [`MultiRepoMechanicalWorker`], which let one worker tick a deterministic
//!   set of repositories while reporting per-repo failures and continuing the
//!   rest of the scan.
//! - [`CiSink`] and [`CiWorker`], the test-only outside-world CI producer seam
//!   used to seed native CI jobs for layered scenarios; real deployments rely on
//!   provider CI and only use the engine's read side.
//! - [`FixpointDriver`], [`PollLoop`], [`WakeablePollLoop`], [`IdlePollBackoff`],
//!   and [`Stage`]/[`InProcessStage`]/[`MultiProcessStage`], which compose workers
//!   into deterministic memory, filesystem, and process-split rehearsal worlds
//!   while keeping per-worker Forge identity a handle-construction concern.
//!   Integration-test support
//!   supplies deterministic fake reference-delivery agents behind [`Agent`];
//!   they contain behavior only and
//!   perform workflow mutations solely through [`RoleTools`].
//!
//! The runner owns recovery coordination state. In a single-process composition
//! the command journal value and lease manager live with the worker set. In a
//! multi-process composition the journal is per-process fast-recovery state,
//! while leases remain durable in Forge metadata; a mechanical process can
//! reconstruct from Forge state, so correctness does not depend on an in-memory
//! journal surviving a restart.

pub mod agent;
pub mod cadence;
pub mod coding_workspace;
pub mod config;
pub mod driver;
pub mod multi_repo;
pub mod observability;
pub mod role_decision;
pub mod role_decision_process;
mod role_process_tools;
pub mod scan;
pub mod signal;
pub mod stage;
pub mod trigger;
pub mod worker;
mod workspace_automation;
mod workspace_request;

pub use agent::{Agent, AgentError, AgentRegistry, RoleTools};
pub use cadence::IdlePollBackoff;
pub use coding_workspace::{
    CODING_WORKSPACE_TOOL_ID, CodingWorkspace, CodingWorkspaceError, CodingWorkspaceGuidance,
    CodingWorkspaceOutput, CodingWorkspaceRepository, CodingWorkspaceRequest,
    CodingWorkspaceWorkItem, ExternalToolExecutors, WorkspaceCheckout,
};
pub use config::{
    ExternalToolBinding, ExternalToolBindingError, PullRequestCreateBinding, RoleBinding,
    RunnerConfig,
};
pub use driver::{
    DriveError, FixpointDriver, ManualClock, PollLoop, RunReport, WakeablePollLoop, WorkerRunReport,
};
pub use multi_repo::{
    MultiRepoConfigError, MultiRepoError, MultiRepoMechanicalWorker, MultiRepoRoleWorker,
    MultiRepoTickReport, RepositoryFailure, RepositoryJournal, RepositoryProgress, RepositorySet,
    RepositoryTarget,
};
pub use observability::{
    ObservabilityArtifactType, WorkItemIdentity, artifact_ref, execution_error_diagnostic_classes,
    execution_error_failure_class, labels_delta, postcondition_outcome_for_error, work_item_ref,
    workflow_effect_summary,
};
pub use role_decision::{
    AuthorizedWorkflowAction, BoundExternalTool, WORKFLOW_ROLE_DECISION_NO_ACTION,
    WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION, WorkflowRoleDecisionProtocolError,
    WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};
pub use role_decision_process::{
    WorkflowRoleDecisionProcessAgent, WorkflowRoleDecisionProcessConfig,
    WorkflowRoleDecisionProcessError,
};
pub use scan::{
    AutomatedWorkItem, CandidateQueryPlan, ScanError, ScanMode, WorkItem, candidate_query_plan,
    scan, scan_audit, scan_automated_queues, scan_role, scan_role_audit, scan_role_wake,
};
pub use signal::{CiError, CiPolicy, CiSink, CiWorker, PassCiPolicy};
pub use stage::{
    BoxError, DEFAULT_SCENARIO_BUDGET, InProcessStage, InProcessWorkerContext,
    InProcessWorkerFactory, MultiProcessStage, Scenario, ScenarioError, ScenarioFuture,
    ScenarioStep, Stage, StageError, WorkerProcess, run_scenario, run_scenario_with_budget,
};
pub use trigger::{ChangeHint, ChangeKind, TriggerScheduler, WakeTarget, broad_targets};
pub use worker::{MechanicalWorker, Progress, RoleWorker, Worker, WorkerError};
pub use workspace_request::{pr_branch_hint, pr_correlation_key, workspace_content_key};
