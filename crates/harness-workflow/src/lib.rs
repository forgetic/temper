//! Workflow and orchestration logic for Harness.
//!
//! `harness-workflow` owns workflow policy and orchestration on top of the
//! backend-agnostic Forge interface in `harness-forge`. It must not contain
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
//! prompt, tool, queue, and label manifests plus a runtime transition table. No
//! transition is executed; compilation only projects the validated model.
//!
//! Phase 5 added pure queue evaluation and transition planning ([`plan`]): a
//! [`Planner`] matches classified artifacts against queues and plans transitions
//! into typed [`WorkflowEffect`]s and postconditions without touching a Forge
//! backend.
//!
//! Phase 6 added runtime execution ([`execute`]): an [`Executor`] loads fresh
//! Forge state, re-plans a transition against it, applies the planned effects
//! through the [`harness_forge::Forge`] trait, and verifies postconditions. It
//! also offers idempotent create through correlation keys.
//!
//! Phase 7 added recovery: leases ([`lease`]) with a pure planner and a
//! backend-applying manager, command journaling ([`journal`]) behind a trait
//! with an in-memory implementation, and a reconciler ([`reconcile`]) that scans
//! Forge artifacts and journal entries and decides repair or escalation actions
//! through a [`RecoveryPolicy`].
//!
//! Phase 9 added non-label effects: the spec, planner, and [`Effect`] express
//! assignee, comment, pull-request create, and merge effects (9a); the
//! [`Executor`] applies assignee/comment effects (9b) and merges at most once
//! with post-merge label projection (9c). Phase 10 added idempotent
//! pull-request creation through [`Executor::ensure_pull_request`] and
//! `CreatePullRequest` execution with runtime inputs from [`ExecutionContext`].
//! Phase 11 added external-signal gates over Forge-projected label/state
//! conditions. Phase 12a added first-class relation declarations and typed
//! classification of metadata-projected relations. Phase 13 added queue
//! activation policies for depth- or age-gated servicing. Phase 14 added
//! multi-kind queues and disjunctive queue label filters.

pub mod artifact;
pub mod classify;
pub mod compile;
pub mod context;
pub mod diagnostics;
pub mod execute;
pub mod ids;
pub mod journal;
pub mod lease;
pub mod metadata;
pub mod plan;
pub mod reconcile;
pub mod relation;
pub mod spec;
pub mod validate;
pub mod validated;

pub use artifact::ArtifactTarget;
pub use classify::{
    ArtifactSource, ClassificationDiagnostic, ClassificationError, ClassifiedArtifact,
    ClassifiedRelation, Classifier,
};
pub use compile::{
    compile, CompiledWorkflow, LabelManifest, LabelSpec, LabelUsage, PromptManifest, PromptSection,
    QueueManifest, RoleManifest, ToolManifest, TransitionManifest,
};
pub use context::ExecutionContext;
pub use diagnostics::{Diagnostic, ReferenceSite, Severity, SymbolKind, ValidationErrors};
pub use execute::{EnsureOutcome, ExecutionError, ExecutionReport, Executor};
pub use ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};
pub use journal::{
    CommandId, CommandJournal, CommandRecord, CommandState, InMemoryJournal, JournalError,
};
pub use lease::{LeaseConflict, LeaseError, LeaseManager, LeasePlanner, LeasePolicy};
pub use metadata::{
    parse_metadata_block, render_metadata_block, replace_metadata_block, Lease, MetadataError,
    WorkflowMetadata, METADATA_BEGIN, METADATA_END,
};
pub use plan::{
    matches_queue, queue_active, PlanDiagnostic, PlanError, Planner, Postcondition, QueueMember,
    QueueQuery, TransitionPlan, WorkflowEffect,
};
pub use reconcile::{
    ArtifactSnapshot, DefaultRecoveryPolicy, ReconcileError, ReconcileFinding, ReconcileReport,
    Reconciler, RecoveryAction, RecoveryPolicy,
};
pub use relation::RelationKind;
pub use spec::{
    RawArtifactKind, RawEffect, RawGate, RawGateCondition, RawLabel, RawQueue, RawQueueLabelSet,
    RawRelation, RawRole, RawState, RawStateDimension, RawTransition, RawWorkflowSpec,
};
pub use validate::validate;
pub use validated::{
    Effect, GateCondition, QueueLabelSet, ValidatedArtifactKind, ValidatedGate, ValidatedQueue,
    ValidatedRelation, ValidatedRole, ValidatedState, ValidatedStateDimension, ValidatedTransition,
    ValidatedWorkflow,
};
