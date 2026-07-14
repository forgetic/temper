// SPDX-License-Identifier: MPL-2.0

use temper_protocol_activity::{AgentActivityEventV1, AgentRunEventV1, CapturedContentV1};

use crate::trace_journal::AgentTraceRun;

use super::model::{TraceRunCounts, TraceRunIdentity, TraceRunSummary};

pub(super) fn project_summary(run: &AgentTraceRun) -> TraceRunSummary {
    let mut counts = TraceRunCounts {
        events: run.events.len() as u64,
        ..TraceRunCounts::default()
    };
    let mut has_trace_gaps = false;
    let mut has_truncated_content = run.summary.quota_exceeded_for_required_boundaries;
    let mut terminal_duration = None;
    for event in &run.events {
        match &event.event {
            AgentActivityEventV1::ScopeStarted(_) => counts.scopes += 1,
            AgentActivityEventV1::TurnStarted(_) => counts.turns += 1,
            AgentActivityEventV1::ModelCallStarted(_) => counts.model_calls += 1,
            AgentActivityEventV1::ToolStarted(_) => counts.tool_calls += 1,
            AgentActivityEventV1::ModelCallRetrying(_) => counts.retries += 1,
            AgentActivityEventV1::TraceGap(_) => has_trace_gaps = true,
            AgentActivityEventV1::RunFinished(finished) => {
                terminal_duration = Some(finished.duration_ms);
            }
            _ => {}
        }
        has_truncated_content |= event_has_truncated_content(event);
    }
    let duration_ms = terminal_duration.or_else(|| {
        let first = run.events.first()?;
        let last = run.events.last()?;
        Some(last.elapsed_ms.saturating_sub(first.elapsed_ms))
    });

    TraceRunSummary {
        version: 1,
        run_id: run.manifest.run_id.clone(),
        identity: TraceRunIdentity {
            worker_id: run.manifest.worker_id.clone(),
            assignment_id: run.manifest.assignment_id.clone(),
            job_id: run.manifest.assignment.job_id.clone(),
            repository: run.manifest.assignment.repository.clone(),
            artifact_ref: run.manifest.assignment.artifact_ref.clone(),
            role: run.manifest.assignment.role.clone(),
            action: run.manifest.assignment.action.clone(),
            correlation_key: run.manifest.assignment.correlation_key.clone(),
            agent_session_id: run.manifest.agent_session_id.clone(),
        },
        status: run.summary.status,
        started_at: run.summary.started_at.clone(),
        completed_at: run.summary.completed_at.clone(),
        duration_ms,
        counts,
        usage: run.summary.usage.clone(),
        capture_mode: run.manifest.capture_policy.capture,
        has_truncated_content,
        has_trace_gaps,
        dropped_events: run.summary.dropped_events,
        first_seq: run.summary.first_seq,
        last_seq: run.summary.last_accepted_seq,
    }
}

fn event_has_truncated_content(event: &AgentRunEventV1) -> bool {
    match &event.event {
        AgentActivityEventV1::AssistantMessage(message) => content_is_truncated(&message.content),
        AgentActivityEventV1::OutputTextDelta(delta)
        | AgentActivityEventV1::OutputThinkingDelta(delta) => delta.delta.truncated,
        AgentActivityEventV1::ToolStarted(tool) => {
            tool.arguments.as_ref().is_some_and(content_is_truncated)
        }
        AgentActivityEventV1::ToolFinished(tool) => {
            tool.result.as_ref().is_some_and(content_is_truncated)
        }
        AgentActivityEventV1::SteeringApplied(steering) => steering
            .instruction
            .as_ref()
            .is_some_and(content_is_truncated),
        _ => false,
    }
}

fn content_is_truncated(content: &CapturedContentV1) -> bool {
    matches!(content, CapturedContentV1::Inline(inline) if inline.truncated)
}
