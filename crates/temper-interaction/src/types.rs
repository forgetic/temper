use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::error::InteractionError;
use crate::proposal::Proposal;

/// Deterministic slug rule shared by conversation profile ids and proposal ids.
pub const DETERMINISTIC_SLUG_RULE: &str =
    "use 1-80 lowercase ASCII letters or digits separated by single hyphens";

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

/// Validates a deterministic id/slug and returns a domain error on failure.
pub fn validate_deterministic_slug(
    field: &'static str,
    value: &str,
) -> Result<(), InteractionError> {
    if is_valid_deterministic_slug(value) {
        Ok(())
    } else {
        Err(InteractionError::InvalidSlug {
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
pub fn validate_proposal_slug(value: &str) -> Result<(), InteractionError> {
    validate_deterministic_slug("proposal id", value)
}

macro_rules! deterministic_id {
    ($name:ident, $field:literal) => {
        #[doc = concat!("Typed deterministic identifier for ", $field, ".")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Builds a validated ", $field, ".")]
            pub fn new(value: impl Into<String>) -> Result<Self, InteractionError> {
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
            type Err = InteractionError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InteractionError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = InteractionError;

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
///
/// This is domain data only and is suitable for JSON exchange with a future
/// external responder process.
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
///
/// This is domain data only and is suitable for JSON exchange with a future
/// external responder process. Proposals are inert until an acceptance service
/// validates and applies them.
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
}
