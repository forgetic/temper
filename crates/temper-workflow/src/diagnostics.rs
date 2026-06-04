//! Validation diagnostics.
//!
//! Validation collects every problem it finds rather than failing on the first
//! one. A [`Diagnostic`] describes a single problem; [`ValidationErrors`] is the
//! error type returned when at least one error-severity diagnostic is present.

use std::error::Error;
use std::fmt;

/// Severity of a diagnostic.
///
/// Phase 2 emits only [`Severity::Error`] diagnostics. The variant set is
/// reserved so later phases can add warnings (for example, unreachable queues
/// or declared-but-unused labels) without changing the diagnostic shape.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Severity {
    /// A problem that prevents producing a `ValidatedWorkflow`.
    Error,
    /// A non-fatal concern that does not block validation.
    Warning,
}

/// The kind of workflow symbol a diagnostic refers to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SymbolKind {
    Role,
    Label,
    ArtifactKind,
    StateDimension,
    State,
    Queue,
    Transition,
    Gate,
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            SymbolKind::Role => "role",
            SymbolKind::Label => "label",
            SymbolKind::ArtifactKind => "artifact kind",
            SymbolKind::StateDimension => "state dimension",
            SymbolKind::State => "state",
            SymbolKind::Queue => "queue",
            SymbolKind::Transition => "transition",
            SymbolKind::Gate => "gate",
        };
        formatter.write_str(text)
    }
}

/// Where an undeclared reference was found.
///
/// Each variant names the declaring symbol so diagnostics point at the exact
/// location of a dangling reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReferenceSite {
    /// A role's `queues` list referenced a queue.
    RoleQueue { role: String },
    /// A transition's `roles` list referenced a role.
    TransitionRole { transition: String },
    /// A transition's `artifact` referenced an artifact kind.
    TransitionArtifact { transition: String },
    /// A transition's `requires_gates` list referenced a gate.
    TransitionGate { transition: String },
    /// A transition effect referenced a label.
    TransitionEffectLabel { transition: String },
    /// A transition effect referenced a role.
    TransitionEffectRole { transition: String },
    /// A queue's `artifact` referenced an artifact kind.
    QueueArtifact { queue: String },
    /// A queue's `labels` list referenced a label.
    QueueLabel { queue: String },
    /// A queue's condition referenced a label or state.
    QueueCondition { queue: String },
    /// A queue automation block referenced an actor role.
    QueueAutomationActor { queue: String },
    /// A queue automation block referenced its primary transition.
    QueueAutomationTransition { queue: String },
    /// A queue automation block referenced its merge-conflict fallback transition.
    QueueAutomationConflictFallback { queue: String },
    /// An artifact kind's `labels` list referenced a label.
    ArtifactLabel { artifact: String },
    /// A state's `label` referenced a label.
    StateLabel { dimension: String, state: String },
    /// A state's `artifacts` list referenced an artifact kind.
    StateArtifact { dimension: String, state: String },
    /// A gate's `satisfied_by` list referenced a transition.
    GateTransition { gate: String },
    /// A relation's source endpoint referenced an artifact kind.
    RelationSource { relation: String },
    /// A relation's target endpoint referenced an artifact kind.
    RelationTarget { relation: String },
    /// A gate's external condition referenced a label or state.
    GateCondition { gate: String },
}

impl fmt::Display for ReferenceSite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceSite::RoleQueue { role } => write!(formatter, "role `{role}`"),
            ReferenceSite::TransitionRole { transition } => {
                write!(formatter, "transition `{transition}`")
            }
            ReferenceSite::TransitionArtifact { transition } => {
                write!(formatter, "transition `{transition}`")
            }
            ReferenceSite::TransitionGate { transition } => {
                write!(formatter, "transition `{transition}`")
            }
            ReferenceSite::TransitionEffectLabel { transition }
            | ReferenceSite::TransitionEffectRole { transition } => {
                write!(formatter, "an effect of transition `{transition}`")
            }
            ReferenceSite::QueueArtifact { queue }
            | ReferenceSite::QueueLabel { queue }
            | ReferenceSite::QueueCondition { queue } => write!(formatter, "queue `{queue}`"),
            ReferenceSite::QueueAutomationActor { queue }
            | ReferenceSite::QueueAutomationTransition { queue }
            | ReferenceSite::QueueAutomationConflictFallback { queue } => {
                write!(formatter, "automation for queue `{queue}`")
            }
            ReferenceSite::ArtifactLabel { artifact } => {
                write!(formatter, "artifact kind `{artifact}`")
            }
            ReferenceSite::StateLabel { dimension, state }
            | ReferenceSite::StateArtifact { dimension, state } => {
                write!(formatter, "state `{state}` in dimension `{dimension}`")
            }
            ReferenceSite::RelationSource { relation } => {
                write!(formatter, "source of relation `{relation}`")
            }
            ReferenceSite::RelationTarget { relation } => {
                write!(formatter, "target of relation `{relation}`")
            }
            ReferenceSite::GateTransition { gate } | ReferenceSite::GateCondition { gate } => {
                write!(formatter, "gate `{gate}`")
            }
        }
    }
}

/// A single validation problem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Diagnostic {
    /// Two or more symbols of the same kind share an id.
    DuplicateId { kind: SymbolKind, id: String },
    /// Two or more states within one dimension share an id.
    DuplicateState { dimension: String, id: String },
    /// Two or more external tool declarations within one role share an id.
    DuplicateRoleExternalTool { role: String, id: String },
    /// A reference points at a symbol that was never declared.
    UndeclaredReference {
        expected: SymbolKind,
        id: String,
        site: ReferenceSite,
    },
    /// A queue did not select any artifact kinds.
    EmptyQueueArtifacts { queue: String },
    /// A queue automation transition does not authorize its declared actor.
    QueueAutomationUnauthorized {
        queue: String,
        actor: String,
        transition: String,
    },
    /// A queue automation primary transition acts on an artifact outside the queue.
    QueueAutomationArtifactMismatch {
        queue: String,
        transition: String,
        artifact: String,
        queue_artifacts: Vec<String>,
    },
    /// A queue automation conflict fallback acts on a different artifact kind than the primary transition.
    QueueAutomationConflictFallbackArtifactMismatch {
        queue: String,
        fallback: String,
        expected: String,
        actual: String,
    },
}

impl Diagnostic {
    /// Returns the severity of this diagnostic.
    pub fn severity(&self) -> Severity {
        match self {
            Diagnostic::DuplicateId { .. }
            | Diagnostic::DuplicateState { .. }
            | Diagnostic::DuplicateRoleExternalTool { .. }
            | Diagnostic::UndeclaredReference { .. }
            | Diagnostic::EmptyQueueArtifacts { .. }
            | Diagnostic::QueueAutomationUnauthorized { .. }
            | Diagnostic::QueueAutomationArtifactMismatch { .. }
            | Diagnostic::QueueAutomationConflictFallbackArtifactMismatch { .. } => Severity::Error,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Diagnostic::DuplicateId { kind, id } => {
                write!(formatter, "duplicate {kind} id `{id}`")
            }
            Diagnostic::DuplicateState { dimension, id } => write!(
                formatter,
                "duplicate state id `{id}` in dimension `{dimension}`"
            ),
            Diagnostic::DuplicateRoleExternalTool { role, id } => write!(
                formatter,
                "role `{role}` declares duplicate external tool id `{id}`"
            ),
            Diagnostic::UndeclaredReference { expected, id, site } => {
                write!(formatter, "{site} references undeclared {expected} `{id}`")
            }
            Diagnostic::EmptyQueueArtifacts { queue } => {
                write!(formatter, "queue `{queue}` selects no artifact kinds")
            }
            Diagnostic::QueueAutomationUnauthorized {
                queue,
                actor,
                transition,
            } => write!(
                formatter,
                "automation for queue `{queue}` uses actor `{actor}`, but transition `{transition}` does not authorize that role"
            ),
            Diagnostic::QueueAutomationArtifactMismatch {
                queue,
                transition,
                artifact,
                queue_artifacts,
            } => write!(
                formatter,
                "automation for queue `{queue}` uses transition `{transition}` on artifact `{artifact}`, which is not selected by the queue ({})",
                queue_artifacts.join(", ")
            ),
            Diagnostic::QueueAutomationConflictFallbackArtifactMismatch {
                queue,
                fallback,
                expected,
                actual,
            } => write!(
                formatter,
                "automation for queue `{queue}` uses conflict fallback `{fallback}` on artifact `{actual}`, but the primary transition acts on `{expected}`"
            ),
        }
    }
}

/// Collection of diagnostics returned when validation fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationErrors {
    diagnostics: Vec<Diagnostic>,
}

impl ValidationErrors {
    /// Creates a collection from diagnostics. Intended for use by validation.
    pub(crate) fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }

    /// Returns the collected diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Returns the number of diagnostics.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns `true` if there are no diagnostics.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workflow validation failed with {} diagnostic(s)",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  - {diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for ValidationErrors {}
