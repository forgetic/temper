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

pub mod artifact;
pub mod classify;
pub mod compile;
pub mod diagnostics;
pub mod ids;
pub mod metadata;
pub mod spec;
pub mod validate;
pub mod validated;

pub use artifact::ArtifactTarget;
pub use classify::{
    ArtifactSource, ClassificationDiagnostic, ClassificationError, ClassifiedArtifact, Classifier,
};
pub use compile::{
    compile, CompiledWorkflow, LabelManifest, LabelSpec, LabelUsage, PromptManifest, PromptSection,
    QueueManifest, RoleManifest, ToolManifest, TransitionManifest,
};
pub use diagnostics::{Diagnostic, ReferenceSite, Severity, SymbolKind, ValidationErrors};
pub use ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
};
pub use metadata::{
    parse_metadata_block, render_metadata_block, Lease, MetadataError, WorkflowMetadata,
    METADATA_BEGIN, METADATA_END,
};
pub use spec::{
    RawArtifactKind, RawEffect, RawGate, RawLabel, RawQueue, RawRole, RawState, RawStateDimension,
    RawTransition, RawWorkflowSpec,
};
pub use validate::validate;
pub use validated::{
    Effect, ValidatedArtifactKind, ValidatedGate, ValidatedQueue, ValidatedRole, ValidatedState,
    ValidatedStateDimension, ValidatedTransition, ValidatedWorkflow,
};
