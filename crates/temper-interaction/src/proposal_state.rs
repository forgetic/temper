//! Durable/reconstructable proposal state stored in transcript comments.

use crate::transcript::{parse_marker_value, validate_marker_namespace};
use crate::types::ParticipantKind;
use crate::{ConversationReply, ConversationTurn, InteractionError, Proposal};

const PROPOSAL_SNAPSHOT_MARKER_SUFFIX: &str = "proposals-v1";

/// Renders a hidden marker that stores the latest proposals in an agent reply.
pub fn render_proposal_snapshot_marker(
    marker_namespace: &str,
    proposals: &[Proposal],
) -> Result<String, InteractionError> {
    validate_marker_namespace(marker_namespace)?;
    let snapshot = ProposalSnapshot {
        version: 1,
        proposals: proposals.to_vec(),
    };
    let json = serde_json::to_vec(&snapshot)?;
    Ok(format!(
        "<!-- temper:{marker_namespace}-{PROPOSAL_SNAPSHOT_MARKER_SUFFIX}={} -->",
        encode_hex(&json)
    ))
}

/// Parses the durable proposal snapshot from one agent transcript comment.
pub fn parse_proposal_snapshot_marker(
    marker_namespace: &str,
    body: &str,
) -> Result<Option<Vec<Proposal>>, InteractionError> {
    let key = format!("temper:{marker_namespace}-{PROPOSAL_SNAPSHOT_MARKER_SUFFIX}");
    let Some(value) = parse_marker_value(body, &key) else {
        return Ok(None);
    };
    let json = decode_hex(&value)?;
    let snapshot: ProposalSnapshot = serde_json::from_slice(&json)?;
    if snapshot.version != 1 {
        return Err(InteractionError::InvalidConfig {
            field: "proposal_snapshot",
            message: format!("unsupported proposal snapshot version {}", snapshot.version),
        });
    }
    crate::proposal::validate_proposals(&snapshot.proposals)?;
    Ok(Some(snapshot.proposals))
}

/// Removes a durable proposal snapshot marker from a transcript body before the
/// body is supplied back to responders as conversational context.
pub fn strip_proposal_snapshot_marker(marker_namespace: &str, body: &str) -> String {
    let prefix = format!("<!-- temper:{marker_namespace}-{PROPOSAL_SNAPSHOT_MARKER_SUFFIX}=");
    let mut lines = body.lines().collect::<Vec<_>>();
    lines.retain(|line| !line.trim_start().starts_with(&prefix));
    lines.join("\n").trim_end().to_string()
}

/// Reconstructs the latest durable proposals from recent transcript turns.
pub fn latest_proposals_from_turns(
    marker_namespace: &str,
    turns: &[ConversationTurn],
) -> Result<Vec<Proposal>, InteractionError> {
    let mut latest = Vec::new();
    for turn in turns {
        if turn.participant.kind == ParticipantKind::Agent
            && let Some(proposals) = parse_proposal_snapshot_marker(marker_namespace, &turn.body)? {
                latest = proposals;
            }
    }
    Ok(latest)
}

/// Renders an agent reply plus proposal summaries and a durable hidden proposal
/// snapshot marker.
pub fn render_agent_reply_comment_with_proposals(
    reply: &ConversationReply,
    marker_namespace: &str,
) -> Result<String, InteractionError> {
    let mut body = render_agent_reply_comment(reply);
    if !reply.proposals.is_empty() {
        body.push_str("\n\n");
        body.push_str(&render_proposal_snapshot_marker(
            marker_namespace,
            &reply.proposals,
        )?);
    }
    Ok(body)
}

/// Renders an agent reply plus human-readable proposal summaries.
pub fn render_agent_reply_comment(reply: &ConversationReply) -> String {
    let mut body = reply.message.trim().to_string();
    if !reply.proposals.is_empty() {
        body.push_str("\n\nProposals:\n");
        for (index, proposal) in reply.proposals.iter().enumerate() {
            body.push_str(&format!("[{}] {}\n", index + 1, proposal.title));
            body.push_str(&format!("    id: {}\n", proposal.id));
            body.push_str(&format!("    kind: {}\n", proposal.kind));
            if let Some(summary) = proposal.summary.as_deref().filter(|text| !text.is_empty()) {
                body.push_str(&format!("    summary: {summary}\n"));
            }
        }
    }
    body
}

#[derive(serde::Deserialize, serde::Serialize)]
struct ProposalSnapshot {
    version: u8,
    proposals: Vec<Proposal>,
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(value: &str) -> Result<Vec<u8>, InteractionError> {
    if !value.len().is_multiple_of(2) {
        return Err(InteractionError::InvalidConfig {
            field: "proposal_snapshot",
            message: "hex payload has odd length".into(),
        });
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let raw = value.as_bytes();
    for chunk in raw.chunks_exact(2) {
        let high = hex_digit(chunk[0])?;
        let low = hex_digit(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_digit(byte: u8) -> Result<u8, InteractionError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(InteractionError::InvalidConfig {
            field: "proposal_snapshot",
            message: "hex payload contains non-hex characters".into(),
        }),
    }
}
