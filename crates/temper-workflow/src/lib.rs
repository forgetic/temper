//! Workflow and orchestration logic for Temper.
//!
//! `temper-workflow` owns workflow policy and orchestration on top of the
//! backend-agnostic Forge interface in `temper-forge`. It must not contain
//! concrete Forge backend code or agent-provider code.
//!
//! See `docs/reference/workflow-layer.md` and
//! `docs/adr/0007-workflow-layer-and-agent-compilation.md` for the planned
//! contract and phased implementation.
//!
//! # Type phases
//!
//! The crate models a workflow in deliberately separate phases so that invalid
//! internal usage is hard to express:
//!
//! - [`RawWorkflowSpec`] is the serde-loadable document. It uses plain string
//!   ids and is not trusted.
//! - [`ValidatedWorkflow`] is the normalized, internally consistent workflow.
//!   It is produced only by [`validate`] (or [`RawWorkflowSpec::validate`]); it
//!   has no public constructor.
//!
//! Later phases (compilation and runtime) are intended to require a
//! `ValidatedWorkflow`, never a raw spec, so that duplicate ids and undeclared
//! references are ruled out before any prompt, tool manifest, or transition is
//! produced.
//!
//! # What is implemented
//!
//! Phase 2 added typed ids ([`ids`]), the raw spec ([`spec`]), diagnostics
//! ([`diagnostics`]), the validated model ([`validated`]), and static
//! validation ([`validate`]) covering duplicate ids and undeclared references.
//!
//! Phase 3 added artifact-to-Forge target mapping ([`artifact`]), workflow
//! metadata blocks ([`metadata`]), and classification of Forge issues and pull
//! requests into typed workflow artifacts ([`classify`]).
//!
//! Phase 4 added compilation ([`compile`]) of a `ValidatedWorkflow` into role,
//! prompt, workflow-tool, external-tool, queue, and label manifests plus a
//! runtime transition table. No transition is executed; compilation only
//! projects the validated model.
//!
//! Phase 5 added pure queue evaluation and transition planning ([`plan`]): a
//! [`Planner`] matches classified artifacts against queues and plans transitions
//! into typed [`WorkflowEffect`]s and postconditions without touching a Forge
//! backend.
//!
//! Phase 6 added runtime execution ([`execute`]): an [`Executor`] loads fresh
//! Forge state, re-plans a transition against it, applies the planned effects
//! through the [`temper_forge::Forge`] trait, and verifies postconditions. It
//! also offers idempotent create through correlation keys.
//!
//! Phase 7 added recovery: leases ([`lease`]) with a pure planner and a
//! backend-applying manager, command journaling ([`journal`]) behind a trait
//! with an in-memory implementation, and a reconciler ([`reconcile`]) that scans
//! Forge artifacts and journal entries and decides repair or escalation actions
//! through a [`RecoveryPolicy`]. [`recover::Applier`] then applies a decided
//! report through the executor, lease manager, and journal — idempotently and
//! crash-safely — so the scan→apply loop converges; `Escalate`/`Diagnose` stay
//! advisory.
//!
//! Phase 9 added non-label effects: the spec, planner, and [`Effect`] express
//! assignee, comment, pull-request create, and merge effects (9a); the
//! [`Executor`] applies assignee/comment effects (9b) and merges at most once
//! with post-merge label projection (9c). Phase 10 added idempotent
//! pull-request creation through [`Executor::ensure_pull_request`] and
//! `CreatePullRequest` execution with runtime inputs from [`ExecutionContext`].
//! Phase 11 added external-signal gates over Forge-projected label/state
//! conditions. Phase 12a added first-class relation declarations and typed
//! relation classification; ADR 0015 added native dependency links. Phase 13 added queue
//! activation policies for depth- or age-gated servicing. Phase 14 added
//! multi-kind queues and disjunctive queue label filters. ADR 0016 added native
//! pull-request reviews, review gate signals, and reviewer request/review
//! submission effects. Queue automation metadata now lets compiled manifests
//! identify queues serviced mechanically by declared transition authority without
//! changing pure queue matching.
//!
//! Lease acquisition is now a compare-and-swap built on the portable
//! optimistic-concurrency primitive in `temper-forge` (the `Version` token and
//! `expected_version` precondition; see ADR 0013). [`LeaseManager`] captures the
//! artifact version at load and writes leases conditionally
//! ([`LeaseManager::prepare_acquire`] + [`LeaseManager::commit`]), so two
//! acquirers over the same "no lease" snapshot cannot both win; the loser
//! observes [`LeaseError::Contended`].

pub mod artifact;
pub mod assignment_convergence;
pub mod classify;
pub mod compile;
pub mod context;
pub mod dependency_state;
pub mod diagnostics;
pub mod execute;
pub mod ids;
pub mod interest;
mod interrupted_ci;
pub mod journal;
pub mod lease;
pub mod load;
pub mod metadata;
mod missing_ci;
pub mod plan;
mod prompt;
pub mod reconcile;
pub mod recover;
pub mod relation;
mod repair_effects;
pub mod spec;
pub mod validate;
mod validate_build;
pub mod validated;

pub use artifact::{ArtifactRef, ArtifactTarget, NEEDS_HUMAN_LABEL, requires_human_attention};
pub use assignment_convergence::{
    ASSIGNMENT_RECOVERY_AUDIT_MARKER, AssignmentConvergenceError, AssignmentConvergenceOutcome,
    AssignmentConverger, AssignmentValidation,
    recover_advanced_pull_request_assignment_from_durable,
};
pub use classify::{
    ArtifactSource, ClassificationDiagnostic, ClassificationError, ClassifiedArtifact,
    ClassifiedRelation, Classifier,
};
pub use compile::{
    CompiledWorkflow, ExternalToolManifest, LabelManifest, LabelSpec, LabelUsage, PromptManifest,
    PromptSection, QueueManifest, RoleManifest, ToolManifest, TransitionManifest,
    ValidationBindingManifest, compile,
};
pub use context::{CreateIssuesChild, ExecutionContext, TransitionCompletionAudit};
pub use diagnostics::{Diagnostic, ReferenceSite, Severity, SymbolKind, ValidationErrors};
pub use execute::{
    ChildIssueCheckpoint, ChildIssueLifecycleHook, CorrelationLookupPlan, EnsureOutcome,
    ExecutionError, ExecutionReport, Executor, find_pull_request_by_correlation,
    validate_pull_request_topology,
};
pub use ids::{
    ArtifactKindId, ExternalToolId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId,
    TransitionId, ValidationBindingId, VerdictId,
};
pub use interest::{WorkflowInterest, workflow_interest};
pub use interrupted_ci::{InterruptedCiDiagnosticState, InterruptedCiRecoveryState};
pub use journal::{
    CommandId, CommandJournal, CommandRecord, CommandState, InMemoryJournal, JournalError,
};
pub use lease::{
    AssignmentClaimRequest, AssignmentMutation, LeaseConflict, LeaseError, LeaseManager,
    LeasePlanner, LeasePolicy, PreparedAcquire, RecoveredHeartbeatOutcome,
    RecoveredOwnershipLossReason,
};
pub use load::{
    WorkflowDocument, WorkflowLoadError, load_workflow, load_workflow_document, load_workflow_spec,
    parse_workflow_document, parse_workflow_spec,
};
pub use metadata::{
    CreateIssueIntentChild, CreateIssuesCompletion, CreateIssuesIntent, DurableAssignment,
    ExactHeadValidationAuthority, Lease, MAX_PROVIDER_RECOVERY_COUNT,
    MAX_PROVIDER_RECOVERY_ID_BYTES, METADATA_BEGIN, METADATA_END, MetadataBlockInspection,
    MetadataBlockSpan, MetadataError, ProviderRecovery, ProviderRecoveryDisposition,
    ProviderRecoveryFacts, WorkflowMetadata, WorkflowMetadataKey, global_child_correlation_key,
    inspect_metadata_blocks, is_heartbeat_only_body_change, parse_metadata_block,
    render_metadata_block, replace_metadata_block, split_metadata_block,
};
pub use missing_ci::MissingCiRecoveryState;
pub use plan::{
    CiState, CiStatus, CiTerminalEvidence, DependencyReadFailure, DependencyStatus, GateSignals,
    MechanicalPlan, PlanDiagnostic, PlanError, Planner, Postcondition, QueueMember, QueueQuery,
    ReviewStatus, SignalNeeds, TransitionPlan, WorkflowEffect, matches_queue, matches_queue_cheap,
    matches_queue_condition, queue_active,
};
pub use reconcile::{
    ArtifactSnapshot, DefaultRecoveryPolicy, ReconcileError, ReconcileFinding, ReconcileReport,
    Reconciler, ReconciliationCandidateQueryPlan, ReconciliationDetailCache,
    ReconciliationDetailCachePolicy, ReconciliationDetailCacheStats, ReconciliationMode,
    RecoveryAction, RecoveryPolicy, reconciliation_candidate_query_plan,
};
pub use recover::{Applier, ApplyError, ApplyOutcome};
pub use relation::RelationKind;
pub use spec::{
    RawArtifactKind, RawChildKindRequirement, RawEffect, RawExternalTool, RawGate,
    RawGateCondition, RawIntakeAuthor, RawLabel, RawQueue, RawQueueAction, RawQueueAutomation,
    RawQueueLabelSet, RawRelation, RawRole, RawRolePrompt, RawState, RawStateDimension,
    RawTransition, RawValidationBinding, RawValidationBindingDetail, RawWorkflowSpec,
    TargetBranchPolicy,
};
pub use validate::validate;
pub use validated::{
    ChildKindRequirement, Effect, ExternalToolDeclaration, GateCondition, IntakeAuthor,
    QueueAction, QueueAutomation, QueueLabelSet, RolePromptExtension, ValidatedArtifactKind,
    ValidatedGate, ValidatedQueue, ValidatedRelation, ValidatedRole, ValidatedState,
    ValidatedStateDimension, ValidatedTransition, ValidatedValidationBinding, ValidatedWorkflow,
    ValidationBindingDetail,
};
