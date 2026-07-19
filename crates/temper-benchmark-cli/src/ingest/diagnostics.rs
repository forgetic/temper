// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, BlobReferenceV1, CapturedContentV1,
};

use super::TraceIngestError;
use crate::{DiagnosticSeverityV1, TraceDiagnosticCodeV1, TraceDiagnosticV1};

pub(super) fn record_call_diagnostics(
    events: &[AgentRunEventV1],
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<(), TraceIngestError> {
    let mut model = BTreeMap::<(String, u32), u64>::new();
    let mut tools = BTreeMap::<String, (String, u64)>::new();
    for event in events {
        match &event.event {
            AgentActivityEventV1::ModelCallStarted(call) => {
                let key = (call.call_id.clone(), call.attempt);
                if model.insert(key.clone(), event.seq).is_some() {
                    return Err(TraceIngestError::InvalidStream(format!(
                        "model call {} attempt {} starts more than once",
                        key.0, key.1
                    )));
                }
            }
            AgentActivityEventV1::ModelCallFinished(call) => {
                if model
                    .remove(&(call.call_id.clone(), call.attempt))
                    .is_none()
                {
                    diagnostics.push(warning(
                        TraceDiagnosticCodeV1::IncompleteModelCall,
                        format!(
                            "model call {} attempt {} finished without an observed start",
                            call.call_id, call.attempt
                        ),
                        Some(event.seq),
                    ));
                }
            }
            AgentActivityEventV1::ToolStarted(call) => {
                if tools
                    .insert(call.call_id.clone(), (call.name.clone(), event.seq))
                    .is_some()
                {
                    return Err(TraceIngestError::InvalidStream(format!(
                        "tool call {} starts more than once",
                        call.call_id
                    )));
                }
            }
            AgentActivityEventV1::ToolFinished(call) => {
                finish_tool_call(call, event.seq, &mut tools, diagnostics)?;
            }
            _ => {}
        }
    }
    record_unfinished_calls(model, tools, diagnostics);
    Ok(())
}

fn finish_tool_call(
    call: &temper_protocol_activity::ToolFinishedV1,
    seq: u64,
    tools: &mut BTreeMap<String, (String, u64)>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<(), TraceIngestError> {
    match tools.remove(&call.call_id) {
        Some((name, _)) if name == call.name => Ok(()),
        Some((name, _)) => Err(TraceIngestError::InvalidStream(format!(
            "tool call {} changes name from {name} to {}",
            call.call_id, call.name
        ))),
        None => {
            diagnostics.push(warning(
                TraceDiagnosticCodeV1::IncompleteToolCall,
                format!(
                    "tool call {} ({}) finished without an observed start",
                    call.call_id, call.name
                ),
                Some(seq),
            ));
            Ok(())
        }
    }
}

fn record_unfinished_calls(
    model: BTreeMap<(String, u32), u64>,
    tools: BTreeMap<String, (String, u64)>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) {
    for ((call_id, attempt), seq) in model {
        diagnostics.push(warning(
            TraceDiagnosticCodeV1::IncompleteModelCall,
            format!("model call {call_id} attempt {attempt} has no finish event"),
            Some(seq),
        ));
    }
    for (call_id, (name, seq)) in tools {
        diagnostics.push(warning(
            TraceDiagnosticCodeV1::IncompleteToolCall,
            format!("tool call {call_id} ({name}) has no finish event"),
            Some(seq),
        ));
    }
}

pub(super) fn record_truncated_content(
    event: &AgentRunEventV1,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) {
    let truncated = match &event.event {
        AgentActivityEventV1::OutputTextDelta(value)
        | AgentActivityEventV1::OutputThinkingDelta(value) => value.delta.truncated,
        _ => captured_contents(event)
            .into_iter()
            .any(|content| matches!(content, CapturedContentV1::Inline(value) if value.truncated)),
    };
    if truncated {
        diagnostics.push(warning(
            TraceDiagnosticCodeV1::TruncatedContent,
            "captured event content was truncated".to_string(),
            Some(event.seq),
        ));
    }
}

pub(super) fn record_evidence_diagnostics(diagnostics: &mut Vec<TraceDiagnosticV1>) {
    diagnostics.push(info(
        TraceDiagnosticCodeV1::HostEvidenceUnavailable,
        "offline traces do not contain host metadata".to_string(),
    ));
    diagnostics.push(info(
        TraceDiagnosticCodeV1::DiffEvidenceUnavailable,
        "offline traces do not contain final workspace diff evidence".to_string(),
    ));
    diagnostics.push(info(
        TraceDiagnosticCodeV1::ValidationEvidenceUnavailable,
        "offline traces do not contain host-side validation evidence".to_string(),
    ));
}

pub(super) fn content_references(event: &AgentRunEventV1) -> Vec<&BlobReferenceV1> {
    captured_contents(event)
        .into_iter()
        .filter_map(|content| match content {
            CapturedContentV1::Blob { blob } => Some(blob),
            CapturedContentV1::Inline(_) => None,
        })
        .collect()
}

fn captured_contents(event: &AgentRunEventV1) -> Vec<&CapturedContentV1> {
    match &event.event {
        AgentActivityEventV1::PromptPrepared(value) => value.content.iter().collect(),
        AgentActivityEventV1::AssistantMessage(value) => vec![&value.content],
        AgentActivityEventV1::ToolStarted(value) => value.arguments.iter().collect(),
        AgentActivityEventV1::ToolFinished(value) => value.result.iter().collect(),
        AgentActivityEventV1::SteeringApplied(value) => value.instruction.iter().collect(),
        _ => Vec::new(),
    }
}

pub(super) fn warning(
    code: TraceDiagnosticCodeV1,
    message: String,
    seq: Option<u64>,
) -> TraceDiagnosticV1 {
    TraceDiagnosticV1 {
        code,
        severity: DiagnosticSeverityV1::Warning,
        message,
        seq,
    }
}

fn info(code: TraceDiagnosticCodeV1, message: String) -> TraceDiagnosticV1 {
    TraceDiagnosticV1 {
        code,
        severity: DiagnosticSeverityV1::Info,
        message,
        seq: None,
    }
}
