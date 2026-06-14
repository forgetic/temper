//! Diagnostic types for interaction spec validation.

use std::error::Error;
use std::fmt;

/// Severity of an interaction spec diagnostic.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionSpecSeverity {
    /// A problem that prevents producing a validated interaction spec.
    Error,
    /// Reserved for future non-fatal diagnostics.
    Warning,
}

/// The kind of interaction symbol a diagnostic refers to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionSpecSymbolKind {
    /// Interaction spec document id.
    Spec,
    /// Interactive profile id.
    Profile,
    /// Responder declaration id.
    Responder,
    /// Proposal kind id.
    ProposalKind,
    /// Transport command id.
    Command,
    /// Acceptance action id.
    AcceptanceAction,
    /// Marker namespace slug.
    MarkerNamespace,
}

impl fmt::Display for InteractionSpecSymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Spec => "interaction spec",
            Self::Profile => "interactive profile",
            Self::Responder => "responder",
            Self::ProposalKind => "proposal kind",
            Self::Command => "command",
            Self::AcceptanceAction => "acceptance action",
            Self::MarkerNamespace => "marker namespace",
        };
        formatter.write_str(text)
    }
}

/// Where an undeclared reference was found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionSpecReferenceSite {
    /// A profile selected a responder id.
    ProfileResponder { profile: String },
    /// A command action referenced a proposal kind.
    CommandProposalKind { profile: String, command: String },
    /// A command action referenced an acceptance action.
    CommandAcceptanceAction { profile: String, command: String },
    /// An acceptance action referenced a proposal kind.
    AcceptanceProposalKind {
        profile: String,
        acceptance_action: String,
    },
    /// An explicit acceptance policy referenced a command id.
    AcceptancePolicyCommand {
        profile: String,
        acceptance_action: String,
    },
}

impl fmt::Display for InteractionSpecReferenceSite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileResponder { profile } => write!(formatter, "profile `{profile}`"),
            Self::CommandProposalKind { profile, command }
            | Self::CommandAcceptanceAction { profile, command } => {
                write!(formatter, "command `{command}` in profile `{profile}`")
            }
            Self::AcceptanceProposalKind {
                profile,
                acceptance_action,
            }
            | Self::AcceptancePolicyCommand {
                profile,
                acceptance_action,
            } => write!(
                formatter,
                "acceptance action `{acceptance_action}` in profile `{profile}`"
            ),
        }
    }
}

/// A single interaction spec validation diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionSpecDiagnostic {
    /// Two or more symbols of the same kind share an id.
    DuplicateId {
        /// Symbol kind.
        kind: InteractionSpecSymbolKind,
        /// Profile-local scope for profile child symbols.
        profile: Option<String>,
        /// Repeated id.
        id: String,
    },
    /// A deterministic id or marker namespace failed the slug rule.
    InvalidSlug {
        /// Symbol kind.
        kind: InteractionSpecSymbolKind,
        /// Profile-local scope when relevant.
        profile: Option<String>,
        /// Rejected value.
        value: String,
        /// Stable slug rule.
        reason: &'static str,
    },
    /// A reference points at a symbol that was never declared.
    UndeclaredReference {
        /// Expected symbol kind.
        expected: InteractionSpecSymbolKind,
        /// Referenced id.
        id: String,
        /// Reference location.
        site: InteractionSpecReferenceSite,
    },
    /// A transcript policy declared no labels.
    EmptyTranscriptLabels { profile: String },
    /// A transcript policy included an empty label.
    EmptyTranscriptLabel { profile: String },
    /// A transcript title prefix was empty.
    EmptyTranscriptTitlePrefix { profile: String },
    /// A transcript marker namespace was empty.
    EmptyTranscriptMarkerNamespace { profile: String },
    /// A command alias was empty or whitespace.
    EmptyCommandAlias { profile: String, command: String },
    /// Two aliases normalize to the same command string within one profile.
    ConflictingCommandAlias {
        /// Profile containing the conflict.
        profile: String,
        /// Conflicting alias.
        alias: String,
        /// First command that declared the alias.
        first_command: String,
        /// Later command that declared the alias.
        second_command: String,
    },
    /// A transcript target is not supported by this phase.
    UnsupportedTranscriptTarget { profile: String, target: String },
    /// A transcript label policy is not supported by this phase.
    UnsupportedTranscriptLabelPolicy { profile: String, policy: String },
    /// A responder protocol/version is not supported by this phase.
    UnsupportedResponderProtocol { responder: String, protocol: String },
    /// A proposal payload contract is not supported by this phase.
    UnsupportedPayloadContract {
        profile: String,
        proposal_kind: String,
        payload: String,
    },
    /// An acceptance policy is not supported by this phase.
    UnsupportedAcceptancePolicy {
        profile: String,
        acceptance_action: String,
        policy: String,
    },
    /// An acceptance action effect kind is not in the closed effect set.
    UnsupportedEffectKind {
        profile: String,
        acceptance_action: String,
        kind: String,
    },
    /// A required field inside an accepted-action declaration is empty.
    EmptyAcceptanceField {
        profile: String,
        acceptance_action: String,
        field: &'static str,
    },
}

impl InteractionSpecDiagnostic {
    /// Returns diagnostic severity.
    pub fn severity(&self) -> InteractionSpecSeverity {
        InteractionSpecSeverity::Error
    }
}

impl fmt::Display for InteractionSpecDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId { kind, profile, id } => {
                if let Some(profile) = profile {
                    write!(
                        formatter,
                        "duplicate {kind} id `{id}` in profile `{profile}`"
                    )
                } else {
                    write!(formatter, "duplicate {kind} id `{id}`")
                }
            }
            Self::InvalidSlug {
                kind,
                profile,
                value,
                reason,
            } => {
                if let Some(profile) = profile {
                    write!(
                        formatter,
                        "invalid {kind} `{value}` in profile `{profile}`: {reason}"
                    )
                } else {
                    write!(formatter, "invalid {kind} `{value}`: {reason}")
                }
            }
            Self::UndeclaredReference { expected, id, site } => {
                write!(formatter, "{site} references undeclared {expected} `{id}`")
            }
            Self::EmptyTranscriptLabels { profile } => {
                write!(
                    formatter,
                    "profile `{profile}` transcript labels must not be empty"
                )
            }
            Self::EmptyTranscriptLabel { profile } => write!(
                formatter,
                "profile `{profile}` transcript labels must not contain empty values"
            ),
            Self::EmptyTranscriptTitlePrefix { profile } => write!(
                formatter,
                "profile `{profile}` transcript title_prefix must not be empty"
            ),
            Self::EmptyTranscriptMarkerNamespace { profile } => write!(
                formatter,
                "profile `{profile}` transcript marker_namespace must not be empty"
            ),
            Self::EmptyCommandAlias { profile, command } => write!(
                formatter,
                "command `{command}` in profile `{profile}` has an empty alias"
            ),
            Self::ConflictingCommandAlias {
                profile,
                alias,
                first_command,
                second_command,
            } => write!(
                formatter,
                "alias `{alias}` in profile `{profile}` is declared by commands `{first_command}` and `{second_command}`"
            ),
            Self::UnsupportedTranscriptTarget { profile, target } => write!(
                formatter,
                "profile `{profile}` transcript target `{target}` is unsupported"
            ),
            Self::UnsupportedTranscriptLabelPolicy { profile, policy } => write!(
                formatter,
                "profile `{profile}` transcript label policy `{policy}` is unsupported"
            ),
            Self::UnsupportedResponderProtocol {
                responder,
                protocol,
            } => write!(
                formatter,
                "responder `{responder}` protocol `{protocol}` is unsupported"
            ),
            Self::UnsupportedPayloadContract {
                profile,
                proposal_kind,
                payload,
            } => write!(
                formatter,
                "proposal kind `{proposal_kind}` in profile `{profile}` uses unsupported payload contract `{payload}`"
            ),
            Self::UnsupportedAcceptancePolicy {
                profile,
                acceptance_action,
                policy,
            } => write!(
                formatter,
                "acceptance action `{acceptance_action}` in profile `{profile}` uses unsupported acceptance policy `{policy}`"
            ),
            Self::UnsupportedEffectKind {
                profile,
                acceptance_action,
                kind,
            } => write!(
                formatter,
                "acceptance action `{acceptance_action}` in profile `{profile}` uses unsupported effect kind `{kind}`"
            ),
            Self::EmptyAcceptanceField {
                profile,
                acceptance_action,
                field,
            } => write!(
                formatter,
                "acceptance action `{acceptance_action}` in profile `{profile}` has empty field `{field}`"
            ),
        }
    }
}

/// Error collection returned when interaction spec validation fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionSpecValidationErrors {
    diagnostics: Vec<InteractionSpecDiagnostic>,
}

impl InteractionSpecValidationErrors {
    pub(crate) fn new(diagnostics: Vec<InteractionSpecDiagnostic>) -> Self {
        Self { diagnostics }
    }

    /// Returns the collected diagnostics.
    pub fn diagnostics(&self) -> &[InteractionSpecDiagnostic] {
        &self.diagnostics
    }

    /// Returns the number of diagnostics.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns `true` when no diagnostics were collected.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

impl fmt::Display for InteractionSpecValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "interaction spec validation failed with {} diagnostic(s)",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  - {diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for InteractionSpecValidationErrors {}
