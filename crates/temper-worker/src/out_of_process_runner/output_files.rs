//! Bounded first-party agent output-file consumers.

use std::path::Path;

use temper_protocol_activity::{
    MAX_OPERATOR_TRANSCRIPT_BYTES, MAX_OPERATOR_TRANSCRIPT_RECORDS, OperatorTranscriptToolResultV1,
};
use temper_protocol_agent::{AgentTerminalOutputV1, MAX_AGENT_TERMINAL_OUTPUT_BYTES};

pub(super) fn read_operator_transcript(path: Option<&Path>) -> Vec<OperatorTranscriptToolResultV1> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if !metadata.is_file()
        || metadata.len() > u64::try_from(MAX_OPERATOR_TRANSCRIPT_BYTES).unwrap_or(u64::MAX)
    {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if records.len() >= MAX_OPERATOR_TRANSCRIPT_RECORDS {
            return Vec::new();
        }
        let Ok(record) = serde_json::from_slice::<OperatorTranscriptToolResultV1>(line) else {
            return Vec::new();
        };
        if record.validate().is_err() {
            return Vec::new();
        }
        records.push(record);
    }
    records
}

pub(super) fn first_party_terminal_model_failure(
    first_party: bool,
    path: &Path,
) -> Option<temper_protocol_activity::ModelFailureV1> {
    if !first_party {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.len() > u64::try_from(MAX_AGENT_TERMINAL_OUTPUT_BYTES).unwrap_or(u64::MAX)
    {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mut output: AgentTerminalOutputV1 = serde_json::from_slice(&bytes).ok()?;
    output.validate().ok()?;
    output.model_failure.normalize();
    Some(output.model_failure)
}
