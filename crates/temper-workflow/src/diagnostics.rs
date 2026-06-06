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
    /// A queue automation outcome referenced a transition for a verdict.
    QueueAutomationOutcome { queue: String, verdict: String },
    /// A transition outcome referenced a transition for a verdict.
    TransitionOutcome { transition: String, verdict: String },
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
    /// The workflow's `intake_author` referenced a role.
    IntakeAuthor,
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
            | ReferenceSite::QueueAutomationTransition { queue } => {
                write!(formatter, "automation for queue `{queue}`")
            }
            ReferenceSite::QueueAutomationOutcome { queue, verdict } => {
                write!(
                    formatter,
                    "outcome `{verdict}` of automation for queue `{queue}`"
                )
            }
            ReferenceSite::TransitionOutcome {
                transition,
                verdict,
            } => {
                write!(
                    formatter,
                    "outcome `{verdict}` of transition `{transition}`"
                )
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
            ReferenceSite::IntakeAuthor => write!(formatter, "intake author"),
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
    /// A queue automation declares a workspace executor id that the actor role
    /// does not declare among its external tools.
    QueueAutomationExecutorUndeclared {
        queue: String,
        actor: String,
        executor: String,
    },
    /// A queue automation outcome transition does not authorize the declared actor.
    QueueAutomationOutcomeUnauthorized {
        queue: String,
        verdict: String,
        actor: String,
        transition: String,
    },
    /// A queue automation outcome transition acts on a different artifact kind than the primary transition.
    QueueAutomationOutcomeArtifactMismatch {
        queue: String,
        verdict: String,
        transition: String,
        expected: String,
        actual: String,
    },
    /// A transition outcome routes to a transition that does not authorize the
    /// primary transition's roles.
    TransitionOutcomeUnauthorized {
        transition: String,
        verdict: String,
        outcome_transition: String,
    },
    /// A transition outcome routes to a transition on a different artifact kind
    /// than the primary transition.
    TransitionOutcomeArtifactMismatch {
        transition: String,
        verdict: String,
        outcome_transition: String,
        expected: String,
        actual: String,
    },
    /// More than one artifact kind for the same Forge target declares no
    /// identifying labels. A target may have at most one default (catch-all)
    /// kind, so the engine cannot decide which one admits an unlabeled artifact.
    MultipleDefaultArtifactKinds { target: String, kinds: Vec<String> },
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
            | Diagnostic::QueueAutomationExecutorUndeclared { .. }
            | Diagnostic::QueueAutomationOutcomeUnauthorized { .. }
            | Diagnostic::QueueAutomationOutcomeArtifactMismatch { .. }
            | Diagnostic::TransitionOutcomeUnauthorized { .. }
            | Diagnostic::TransitionOutcomeArtifactMismatch { .. }
            | Diagnostic::MultipleDefaultArtifactKinds { .. } => Severity::Error,
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
            Diagnostic::QueueAutomationExecutorUndeclared {
                queue,
                actor,
                executor,
            } => write!(
                formatter,
                "automation for queue `{queue}` declares workspace executor `{executor}`, but actor role `{actor}` does not declare that external tool"
            ),
            Diagnostic::QueueAutomationOutcomeUnauthorized {
                queue,
                verdict,
                actor,
                transition,
            } => write!(
                formatter,
                "automation for queue `{queue}` routes verdict `{verdict}` to transition `{transition}`, but that transition does not authorize actor `{actor}`"
            ),
            Diagnostic::QueueAutomationOutcomeArtifactMismatch {
                queue,
                verdict,
                transition,
                expected,
                actual,
            } => write!(
                formatter,
                "automation for queue `{queue}` routes verdict `{verdict}` to transition `{transition}` on artifact `{actual}`, but the primary transition acts on `{expected}`"
            ),
            Diagnostic::TransitionOutcomeUnauthorized {
                transition,
                verdict,
                outcome_transition,
            } => write!(
                formatter,
                "transition `{transition}` routes verdict `{verdict}` to transition `{outcome_transition}`, but that transition shares none of the primary transition's roles"
            ),
            Diagnostic::TransitionOutcomeArtifactMismatch {
                transition,
                verdict,
                outcome_transition,
                expected,
                actual,
            } => write!(
                formatter,
                "transition `{transition}` routes verdict `{verdict}` to transition `{outcome_transition}` on artifact `{actual}`, but the primary transition acts on `{expected}`"
            ),
            Diagnostic::MultipleDefaultArtifactKinds { target, kinds } => write!(
                formatter,
                "Forge target `{target}` declares more than one default artifact kind (no identifying labels): {}",
                kinds.join(", ")
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
