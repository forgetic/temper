use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    ConversationId, ConversationProfileId, ConversationTurn, ProposalId, ProposalKind,
};
use crate::{ConversationReply, Proposal};

/// Transport-neutral command for opening a conversation.
///
/// Supplying `transcript_issue` resumes an existing durable transcript; omitting
/// it creates a new conversation. A deployment that exposes only one profile may
/// leave `profile_id` empty and use its configured default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenConversationCommand {
    /// Optional profile to use for this conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ConversationProfileId>,
    /// Optional durable transcript issue number to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_issue: Option<u64>,
    /// Profile-specific context supplied by the deployment, not by the frontend.
    #[serde(default)]
    pub context: Value,
}

impl OpenConversationCommand {
    /// Builds a command that creates a new conversation in the default profile.
    pub fn new() -> Self {
        Self {
            profile_id: None,
            transcript_issue: None,
            context: Value::Object(Default::default()),
        }
    }
}

impl Default for OpenConversationCommand {
    fn default() -> Self {
        Self::new()
    }
}

/// Transport-neutral command for appending one human turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendHumanTurnCommand {
    /// Plain text turn body.
    pub body: String,
}

/// Transport-neutral command for listing the latest inert proposals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListLatestProposalsCommand {
    /// Conversation whose latest proposals should be returned.
    pub conversation_id: ConversationId,
}

/// Transport-neutral command for explicitly accepting one proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptProposalCommand {
    /// Conversation that currently exposes the proposal.
    pub conversation_id: ConversationId,
    /// Stable proposal id to accept.
    pub proposal_id: ProposalId,
}

/// Result of sending one human turn and receiving one agent reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendHumanTurnResult {
    /// Reply returned by the configured interactive responder.
    pub reply: ConversationReply,
    /// Latest proposals after this turn.
    #[serde(default)]
    pub latest_proposals: Vec<Proposal>,
}

/// Minimal transcript reference safe to expose to transports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTranscriptRef {
    /// Durable Forge issue number when the transcript store is Forge-backed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    /// Browser URL for the durable transcript when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl ConversationTranscriptRef {
    /// Builds a Forge issue transcript reference.
    pub fn forge_issue(issue_number: u64, url: impl Into<String>) -> Self {
        Self {
            issue_number: Some(issue_number),
            url: Some(url.into()),
        }
    }
}

/// Snapshot returned by a transport after create/resume or read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    /// Durable conversation id.
    pub id: ConversationId,
    /// Profile selected for this conversation.
    pub profile_id: ConversationProfileId,
    /// Durable transcript reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<ConversationTranscriptRef>,
    /// Recent transcript view available to the frontend.
    #[serde(default)]
    pub turns: Vec<ConversationTurn>,
    /// Latest inert proposals awaiting explicit acceptance.
    #[serde(default)]
    pub latest_proposals: Vec<Proposal>,
}

/// Target created or found by accepting a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedProposalTarget {
    /// Target kind, such as `issue`.
    pub kind: ProposalKind,
    /// Provider-local number when the target is a Forge issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// URL safe to show to a frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl AcceptedProposalTarget {
    /// Builds a target reference for an accepted issue proposal.
    pub fn issue(number: u64, url: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: ProposalKind::issue(),
            number: Some(number),
            url: Some(url.into()),
            title: Some(title.into()),
        }
    }
}

/// Event category for transport-facing conversation updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationEventKind {
    /// A conversation was opened or resumed.
    ConversationOpened,
    /// A human turn was appended to the transcript.
    HumanTurnAppended,
    /// An agent reply was appended to the transcript.
    AgentReplyAppended,
    /// The latest proposal cache changed.
    ProposalsUpdated,
    /// A proposal was explicitly accepted.
    ProposalAccepted,
}

/// Structured event payloads suitable for JSON, SSE, Matrix, web, mobile, or
/// voice adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationEventPayload {
    /// Conversation metadata became available to transports.
    ConversationOpened {
        /// Selected profile.
        profile_id: ConversationProfileId,
        /// Durable transcript reference.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transcript: Option<ConversationTranscriptRef>,
    },
    /// A human transcript turn was recorded.
    HumanTurnAppended {
        /// Recorded turn.
        turn: ConversationTurn,
    },
    /// A responder reply was recorded.
    AgentReplyAppended {
        /// Reply text and proposals returned by the responder.
        reply: ConversationReply,
    },
    /// Latest proposals changed.
    ProposalsUpdated {
        /// Current proposal cache.
        proposals: Vec<Proposal>,
    },
    /// A proposal was accepted by an explicit human command.
    ProposalAccepted {
        /// Accepted proposal id.
        proposal_id: ProposalId,
        /// Whether this command created a new target.
        created: bool,
        /// Existing or created target, when the acceptance path has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<AcceptedProposalTarget>,
    },
}

impl ConversationEventPayload {
    /// Returns the event kind for this payload.
    pub fn kind(&self) -> ConversationEventKind {
        match self {
            ConversationEventPayload::ConversationOpened { .. } => {
                ConversationEventKind::ConversationOpened
            }
            ConversationEventPayload::HumanTurnAppended { .. } => {
                ConversationEventKind::HumanTurnAppended
            }
            ConversationEventPayload::AgentReplyAppended { .. } => {
                ConversationEventKind::AgentReplyAppended
            }
            ConversationEventPayload::ProposalsUpdated { .. } => {
                ConversationEventKind::ProposalsUpdated
            }
            ConversationEventPayload::ProposalAccepted { .. } => {
                ConversationEventKind::ProposalAccepted
            }
        }
    }
}

/// One ordered conversation event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationEvent {
    /// Monotonic sequence number within the in-process event source.
    pub sequence: u64,
    /// Conversation this event belongs to.
    pub conversation_id: ConversationId,
    /// Event category duplicated for simple transport filtering.
    pub kind: ConversationEventKind,
    /// Wall-clock event time.
    pub occurred_at: DateTime<Utc>,
    /// Structured event details.
    pub payload: ConversationEventPayload,
}

impl ConversationEvent {
    fn new(
        sequence: u64,
        conversation_id: ConversationId,
        payload: ConversationEventPayload,
    ) -> Self {
        Self {
            sequence,
            conversation_id,
            kind: payload.kind(),
            occurred_at: Utc::now(),
            payload,
        }
    }
}

/// Testable in-process event source/sink for transport adapters.
///
/// The log is intentionally ephemeral. Durable truth remains in the transcript
/// store and accepted Forge artifacts; this type gives local HTTP/SSE or other
/// adapters a small seam for realtime or replay tests.
#[derive(Clone, Debug, Default)]
pub struct ConversationEventLog {
    inner: Arc<Mutex<EventLogInner>>,
}

impl ConversationEventLog {
    /// Builds an empty event log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one event and assigns a sequence number and timestamp.
    pub fn record(
        &self,
        conversation_id: ConversationId,
        payload: ConversationEventPayload,
    ) -> ConversationEvent {
        let mut inner = self.lock();
        inner.next_sequence += 1;
        let event = ConversationEvent::new(inner.next_sequence, conversation_id.clone(), payload);
        inner
            .by_conversation
            .entry(conversation_id)
            .or_default()
            .push(event.clone());
        event
    }

    /// Lists all currently retained events for a conversation.
    pub fn list(&self, conversation_id: &ConversationId) -> Vec<ConversationEvent> {
        self.lock()
            .by_conversation
            .get(conversation_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Lists retained events after a sequence number.
    pub fn since(
        &self,
        conversation_id: &ConversationId,
        after_sequence: u64,
    ) -> Vec<ConversationEvent> {
        self.list(conversation_id)
            .into_iter()
            .filter(|event| event.sequence > after_sequence)
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, EventLogInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Default)]
struct EventLogInner {
    next_sequence: u64,
    by_conversation: BTreeMap<ConversationId, Vec<ConversationEvent>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{ConversationProfileId, ConversationTurn, Participant, ProposalId};

    use super::*;

    #[test]
    fn command_and_snapshot_json_are_profile_neutral() {
        let command = OpenConversationCommand {
            profile_id: Some(ConversationProfileId::new("intake-assistant").unwrap()),
            transcript_issue: Some(7),
            context: json!({ "deployment": "local" }),
        };
        let encoded = serde_json::to_string(&command).unwrap();
        assert!(encoded.contains("intake-assistant"));
        let decoded: OpenConversationCommand = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, command);

        let snapshot = ConversationSnapshot {
            id: ConversationId::new("conversation-1").unwrap(),
            profile_id: ConversationProfileId::new("intake-assistant").unwrap(),
            transcript: Some(ConversationTranscriptRef::forge_issue(
                7,
                "https://git.example.test/owner/repo/issues/7",
            )),
            turns: vec![ConversationTurn::new(Participant::human("human"), "hello")],
            latest_proposals: Vec::new(),
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["profile_id"], "intake-assistant");
        assert_eq!(value["transcript"]["issue_number"], 7);
    }

    #[test]
    fn event_log_records_replayable_events() {
        let log = ConversationEventLog::new();
        let conversation_id = ConversationId::new("conversation-1").unwrap();
        let opened = log.record(
            conversation_id.clone(),
            ConversationEventPayload::ConversationOpened {
                profile_id: ConversationProfileId::new("intake-assistant").unwrap(),
                transcript: None,
            },
        );
        let accepted = log.record(
            conversation_id.clone(),
            ConversationEventPayload::ProposalAccepted {
                proposal_id: ProposalId::new("file-this").unwrap(),
                created: true,
                target: Some(AcceptedProposalTarget::issue(
                    9,
                    "https://git.example.test/owner/repo/issues/9",
                    "File this",
                )),
            },
        );

        assert_eq!(opened.sequence, 1);
        assert_eq!(accepted.sequence, 2);
        assert_eq!(log.list(&conversation_id).len(), 2);
        assert_eq!(log.since(&conversation_id, 1), vec![accepted]);
    }
}
