//! Minimal JSON conversation/proposal protocol for Temper interactive responders.
//!
//! This crate contains serialization-oriented data transfer objects and
//! validation helpers for interactive responder processes that communicate with
//! Temper only through stdin/stdout JSON. It intentionally has no dependency on
//! Temper runtime, workflow, Forge, backend, or deployment crates.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

/// Deterministic slug rule shared by conversation profile ids and proposal ids.
pub const DETERMINISTIC_SLUG_RULE: &str =
    "use 1-80 lowercase ASCII letters or digits separated by single hyphens";

/// Errors produced while validating interactive responder protocol DTOs.
#[derive(Debug, Error)]
pub enum InteractionProtocolError {
    /// A deterministic id or slug did not match Temper's portable slug rule.
    #[error("invalid {field} `{value}`: {reason}")]
    InvalidSlug {
        /// Human-readable field name, such as `proposal id`.
        field: &'static str,
        /// The rejected value.
        value: String,
        /// The slug rule that was violated.
        reason: &'static str,
    },
    /// A responder returned two proposals with the same stable id.
    #[error("duplicate proposal id `{id}`")]
    DuplicateProposalId {
        /// The repeated proposal id.
        id: ProposalId,
    },
    /// A proposal acceptance path received an unsupported proposal kind.
    #[error("proposal `{id}` has unsupported kind `{kind}`")]
    UnsupportedProposalKind {
        /// Proposal being accepted.
        id: ProposalId,
        /// Actual proposal kind.
        kind: ProposalKind,
    },
    /// Serializing or deserializing profile-specific JSON payload failed.
    #[error("interaction JSON payload failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Returns whether a value matches Temper's deterministic id/slug shape.
///
/// A valid slug is non-empty, at most 80 bytes, and contains lowercase ASCII
/// letters or digits separated by single hyphens. It cannot start or end with a
/// hyphen. The shape is stable and portable; callers are still responsible for
/// choosing deterministic values rather than random timestamps.
pub fn is_valid_deterministic_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 80 {
        return false;
    }

    let mut previous_hyphen = false;
    for (index, byte) in slug.bytes().enumerate() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_hyphen = false,
            b'-' => {
                if index == 0 || index + 1 == slug.len() || previous_hyphen {
                    return false;
                }
                previous_hyphen = true;
            }
            _ => return false,
        }
    }
    true
}

/// Validates a deterministic id/slug and returns a protocol error on failure.
pub fn validate_deterministic_slug(
    field: &'static str,
    value: &str,
) -> Result<(), InteractionProtocolError> {
    if is_valid_deterministic_slug(value) {
        Ok(())
    } else {
        Err(InteractionProtocolError::InvalidSlug {
            field,
            value: value.to_string(),
            reason: DETERMINISTIC_SLUG_RULE,
        })
    }
}

/// Alias for proposal-specific call sites that validate stable proposal ids.
pub fn is_valid_proposal_slug(slug: &str) -> bool {
    is_valid_deterministic_slug(slug)
}

/// Alias for proposal-specific call sites that validate stable proposal ids.
pub fn validate_proposal_slug(value: &str) -> Result<(), InteractionProtocolError> {
    validate_deterministic_slug("proposal id", value)
}

macro_rules! deterministic_id {
    ($name:ident, $field:literal) => {
        #[doc = concat!("Typed deterministic identifier for ", $field, ".")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Builds a validated ", $field, ".")]
            pub fn new(value: impl Into<String>) -> Result<Self, InteractionProtocolError> {
                let value = value.into();
                validate_deterministic_slug($field, &value)?;
                Ok(Self(value))
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identifier and returns the inner string.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = InteractionProtocolError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InteractionProtocolError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = InteractionProtocolError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

deterministic_id!(ConversationProfileId, "conversation profile id");
deterministic_id!(ConversationId, "conversation id");
deterministic_id!(ConversationTurnId, "conversation turn id");
deterministic_id!(ProposalId, "proposal id");
deterministic_id!(ProposalKind, "proposal kind");

impl ProposalKind {
    /// Stable kind used for proposals that can become Forge issues when accepted.
    pub fn issue() -> Self {
        Self("issue".to_string())
    }
}

/// Participant category for a conversation turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantKind {
    /// A human user or operator.
    Human,
    /// An interactive agent responder.
    Agent,
    /// System or runtime context recorded in a transcript.
    System,
}

/// A conversation participant without transport-specific account details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    /// Participant category.
    pub kind: ParticipantKind,
    /// Optional display name or handle safe to show in transcript views.
    pub display_name: Option<String>,
}

impl Participant {
    /// Builds a participant with no display name.
    pub fn new(kind: ParticipantKind) -> Self {
        Self {
            kind,
            display_name: None,
        }
    }

    /// Builds a human participant.
    pub fn human(display_name: impl Into<String>) -> Self {
        Self {
            kind: ParticipantKind::Human,
            display_name: Some(display_name.into()),
        }
    }

    /// Builds an agent participant.
    pub fn agent(display_name: impl Into<String>) -> Self {
        Self {
            kind: ParticipantKind::Agent,
            display_name: Some(display_name.into()),
        }
    }
}

/// One ordered transcript turn passed to an interactive responder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Stable id when the transcript store has one.
    pub id: Option<ConversationTurnId>,
    /// Turn author.
    pub participant: Participant,
    /// Plain text turn body as shown to the responder.
    pub body: String,
}

impl ConversationTurn {
    /// Builds a transcript turn with no store-assigned id.
    pub fn new(participant: Participant, body: impl Into<String>) -> Self {
        Self {
            id: None,
            participant,
            body: body.into(),
        }
    }
}

/// Input for one interactive responder turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationRequest {
    /// Behavior package the responder should run as.
    pub profile_id: ConversationProfileId,
    /// Durable conversation id.
    pub conversation_id: ConversationId,
    /// Ordered transcript view made available to the responder.
    pub turns: Vec<ConversationTurn>,
    /// Profile-specific context, kept JSON-shaped to avoid provider coupling.
    #[serde(default)]
    pub context: Value,
}

impl ConversationRequest {
    /// Builds a request with an empty profile-specific JSON object.
    pub fn new(
        profile_id: ConversationProfileId,
        conversation_id: ConversationId,
        turns: Vec<ConversationTurn>,
    ) -> Self {
        Self {
            profile_id,
            conversation_id,
            turns,
            context: Value::Object(Default::default()),
        }
    }
}

/// Output from one interactive responder turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationReply {
    /// Conversational text to show to the human.
    pub message: String,
    /// Inert proposals that require explicit acceptance before any mutation.
    #[serde(default)]
    pub proposals: Vec<Proposal>,
}

impl ConversationReply {
    /// Builds a reply with no proposals.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            proposals: Vec::new(),
        }
    }

    /// Validates reply proposals for deterministic, unambiguous acceptance.
    pub fn validate(&self) -> Result<(), InteractionProtocolError> {
        validate_proposals(&self.proposals)
    }
}

/// Draft shape for a proposal that can create a normal Forge issue when accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueProposal {
    /// Issue title to use if this proposal is accepted.
    pub title: String,
    /// Issue body to use if this proposal is accepted.
    pub body: String,
    /// Optional human-readable reason the issue is worth filing.
    pub rationale: Option<String>,
}

impl IssueProposal {
    /// Builds an issue proposal with no rationale.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            rationale: None,
        }
    }

    /// Builds an issue proposal with a rationale.
    pub fn with_rationale(
        title: impl Into<String>,
        body: impl Into<String>,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            rationale: Some(rationale.into()),
        }
    }
}

/// A responder-suggested action that remains inert until explicitly accepted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Stable deterministic id used for display, de-duplication, and acceptance.
    pub id: ProposalId,
    /// Typed proposal kind. Profile-specific kinds use the same slug rule.
    pub kind: ProposalKind,
    /// Short display title.
    pub title: String,
    /// Optional display summary or rationale.
    pub summary: Option<String>,
    /// Profile-specific payload for the acceptance path.
    #[serde(default)]
    pub payload: Value,
}

impl Proposal {
    /// Builds a custom proposal with an arbitrary JSON payload.
    pub fn custom(
        id: ProposalId,
        kind: ProposalKind,
        title: impl Into<String>,
        summary: Option<String>,
        payload: Value,
    ) -> Self {
        Self {
            id,
            kind,
            title: title.into(),
            summary,
            payload,
        }
    }

    /// Builds an issue proposal and stores the typed issue draft as JSON payload.
    pub fn issue(id: ProposalId, issue: IssueProposal) -> Result<Self, InteractionProtocolError> {
        let title = issue.title.clone();
        let summary = issue.rationale.clone();
        Ok(Self {
            id,
            kind: ProposalKind::issue(),
            title,
            summary,
            payload: serde_json::to_value(issue)?,
        })
    }

    /// Decodes this proposal's payload as an [`IssueProposal`] when its kind is issue.
    pub fn issue_payload(&self) -> Result<Option<IssueProposal>, InteractionProtocolError> {
        if self.kind != ProposalKind::issue() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(self.payload.clone())?))
    }
}

/// Closed payload validators known to the process protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalPayloadValidator {
    /// Payload must deserialize as an [`IssueProposal`].
    IssueDraft,
}

impl ProposalPayloadValidator {
    /// Validates one proposal payload.
    pub fn validate(&self, proposal: &Proposal) -> Result<(), InteractionProtocolError> {
        match self {
            Self::IssueDraft => {
                let _: IssueProposal = serde_json::from_value(proposal.payload.clone())?;
                Ok(())
            }
        }
    }
}

/// Validates proposal ids and built-in kind payloads in one responder reply.
pub fn validate_proposals(proposals: &[Proposal]) -> Result<(), InteractionProtocolError> {
    validate_proposal_ids(proposals)?;
    for proposal in proposals {
        if proposal.kind == ProposalKind::issue() {
            proposal.issue_payload()?;
        }
    }
    Ok(())
}

/// Rejects duplicate proposal ids in one responder reply.
pub fn validate_proposal_ids(proposals: &[Proposal]) -> Result<(), InteractionProtocolError> {
    let mut seen = HashSet::new();
    for proposal in proposals {
        if !seen.insert(proposal.id.clone()) {
            return Err(InteractionProtocolError::DuplicateProposalId {
                id: proposal.id.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_responder_fixtures_round_trip_and_validate() {
        let request_json = include_str!("../fixtures/interactive-responder-request.json");
        let request: ConversationRequest =
            serde_json::from_str(request_json).expect("process request deserializes");
        let expected_profile = ["product", "manager"].join("-");
        assert_eq!(request.profile_id.as_str(), expected_profile);
        assert_eq!(request.conversation_id.as_str(), "conversation-1");
        assert_eq!(request.turns[0].id.as_ref().unwrap().as_str(), "turn-1");

        let encoded_request = serde_json::to_string(&request).expect("request serializes");
        let decoded_request: ConversationRequest =
            serde_json::from_str(&encoded_request).expect("request round-trips");
        assert_eq!(decoded_request, request);

        let reply_json = include_str!("../fixtures/interactive-responder-reply.json");
        let reply: ConversationReply =
            serde_json::from_str(reply_json).expect("process reply deserializes");
        reply.validate().expect("reply validates");

        let issue = IssueProposal::with_rationale(
            "Add mobile chat adapter",
            "Expose the interaction service through a mobile-friendly adapter.",
            "Mobile access lets humans keep the conversation moving.",
        );
        assert_eq!(
            reply.proposals[0]
                .issue_payload()
                .expect("issue payload decodes"),
            Some(issue)
        );

        let encoded_reply = serde_json::to_string_pretty(&reply).expect("reply serializes");
        assert!(encoded_reply.contains("mobile-chat-adapter"));
        assert!(encoded_reply.contains("issue"));
        let decoded_reply: ConversationReply =
            serde_json::from_str(&encoded_reply).expect("reply round-trips");
        assert_eq!(decoded_reply, reply);
    }

    #[test]
    fn validates_deterministic_proposal_slugs() {
        for slug in ["mvp", "matrix-text-adapter", "api-v1", "a1-b2"] {
            assert!(is_valid_proposal_slug(slug), "{slug} should be valid");
            ProposalId::new(slug).expect("valid proposal id");
            ProposalKind::new(slug).expect("valid proposal kind");
        }

        for slug in [
            "",
            "Matrix",
            "matrix_text",
            "matrix--text",
            "-matrix",
            "matrix-",
            "matrix text",
            "mátřix",
        ] {
            assert!(!is_valid_proposal_slug(slug), "{slug} should be invalid");
            assert!(matches!(
                ProposalId::new(slug),
                Err(InteractionProtocolError::InvalidSlug { .. })
            ));
        }
    }

    #[test]
    fn rejects_duplicate_proposal_ids() {
        let proposal = Proposal::custom(
            ProposalId::new("same-proposal").expect("valid proposal"),
            ProposalKind::new("custom-kind").expect("valid kind"),
            "Duplicate",
            None,
            Value::Null,
        );
        let reply = ConversationReply {
            message: "duplicates".to_string(),
            proposals: vec![proposal.clone(), proposal],
        };
        assert!(matches!(
            reply.validate(),
            Err(InteractionProtocolError::DuplicateProposalId { .. })
        ));
    }
}
