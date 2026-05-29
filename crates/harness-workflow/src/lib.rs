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
//! # What Phase 2 implements
//!
//! Typed ids ([`ids`]), the raw spec ([`spec`]), diagnostics
//! ([`diagnostics`]), the validated model ([`validated`]), and static
//! validation ([`validate`]) covering duplicate ids and undeclared references.

pub mod diagnostics;
pub mod ids;
pub mod spec;
pub mod validate;
pub mod validated;

pub use diagnostics::{Diagnostic, ReferenceSite, Severity, SymbolKind, ValidationErrors};
pub use ids::{
    ArtifactKindId, GateId, LabelId, QueueId, RoleId, StateDimensionId, StateId, TransitionId,
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
