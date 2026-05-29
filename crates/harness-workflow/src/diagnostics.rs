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
    /// An artifact kind's `labels` list referenced a label.
    ArtifactLabel { artifact: String },
    /// A state's `label` referenced a label.
    StateLabel { dimension: String, state: String },
    /// A gate's `satisfied_by` list referenced a transition.
    GateTransition { gate: String },
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
            ReferenceSite::QueueArtifact { queue } => write!(formatter, "queue `{queue}`"),
            ReferenceSite::QueueLabel { queue } => write!(formatter, "queue `{queue}`"),
            ReferenceSite::ArtifactLabel { artifact } => {
                write!(formatter, "artifact kind `{artifact}`")
            }
            ReferenceSite::StateLabel { dimension, state } => {
                write!(formatter, "state `{state}` in dimension `{dimension}`")
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
    /// A reference points at a symbol that was never declared.
    UndeclaredReference {
        expected: SymbolKind,
        id: String,
        site: ReferenceSite,
    },
}

impl Diagnostic {
    /// Returns the severity of this diagnostic.
    pub fn severity(&self) -> Severity {
        match self {
            Diagnostic::DuplicateId { .. }
            | Diagnostic::DuplicateState { .. }
            | Diagnostic::UndeclaredReference { .. } => Severity::Error,
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
            Diagnostic::UndeclaredReference { expected, id, site } => {
                write!(formatter, "{site} references undeclared {expected} `{id}`")
            }
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
