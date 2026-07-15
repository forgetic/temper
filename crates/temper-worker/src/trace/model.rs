use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1,
    BlobReferenceV1, CaptureModeV1, CapturedContentV1, FailureCodeV1, FailureInfoV1, RunFailedV1,
    RunFinishedV1, RunStatusV1, StopReasonV1,
};
use temper_protocol_agent::{ArtifactType, WorkspaceContext};
use temper_protocol_worker::FailureClass;

use super::{TraceError, TraceManifestV1};

pub(super) fn validate_event_policy(
    policy: &AgentActivityCapturePolicyV1,
    event: &AgentActivityEventV1,
) -> Result<(), TraceError> {
    let content = match event {
        AgentActivityEventV1::AssistantMessage(value) => {
            if policy.capture == CaptureModeV1::Metadata {
                return Err(policy_rejection(event));
            }
            Some(&value.content)
        }
        AgentActivityEventV1::OutputTextDelta(value) => {
            if policy.capture != CaptureModeV1::Diagnostic {
                return Err(policy_rejection(event));
            }
            return validate_inline_limit(policy, value.delta.text.len(), event);
        }
        AgentActivityEventV1::OutputThinkingDelta(value) => {
            if policy.capture != CaptureModeV1::Diagnostic || !policy.capture_thinking {
                return Err(policy_rejection(event));
            }
            return validate_inline_limit(policy, value.delta.text.len(), event);
        }
        AgentActivityEventV1::ToolStarted(value) => value.arguments.as_ref(),
        AgentActivityEventV1::ToolFinished(value) => value.result.as_ref(),
        AgentActivityEventV1::SteeringApplied(value) => value.instruction.as_ref(),
        _ => None,
    };
    if policy.capture == CaptureModeV1::Metadata && content.is_some() {
        return Err(policy_rejection(event));
    }
    match content {
        Some(CapturedContentV1::Inline(value)) => {
            validate_inline_limit(policy, value.text.len(), event)
        }
        Some(CapturedContentV1::Blob { blob }) if blob.bytes > policy.max_blob_bytes => {
            Err(policy_rejection(event))
        }
        _ => Ok(()),
    }
}

fn validate_inline_limit(
    policy: &AgentActivityCapturePolicyV1,
    bytes: usize,
    event: &AgentActivityEventV1,
) -> Result<(), TraceError> {
    if u64::try_from(bytes).unwrap_or(u64::MAX) > u64::from(policy.max_inline_bytes) {
        Err(policy_rejection(event))
    } else {
        Ok(())
    }
}

fn policy_rejection(event: &AgentActivityEventV1) -> TraceError {
    TraceError::InvalidSpool(format!(
        "child event {} exceeds the configured capture policy",
        event.event_type()
    ))
}

pub(super) fn event_blob_references(event: &AgentActivityEventV1) -> Vec<&BlobReferenceV1> {
    use temper_protocol_activity::CapturedContentV1;
    let content = match event {
        AgentActivityEventV1::AssistantMessage(value) => Some(&value.content),
        AgentActivityEventV1::ToolStarted(value) => value.arguments.as_ref(),
        AgentActivityEventV1::ToolFinished(value) => value.result.as_ref(),
        AgentActivityEventV1::SteeringApplied(value) => value.instruction.as_ref(),
        _ => None,
    };
    match content {
        Some(CapturedContentV1::Blob { blob }) => vec![blob],
        _ => Vec::new(),
    }
}

pub(super) fn assignment_from_context(
    job_id: &str,
    context: &WorkspaceContext,
) -> AgentAssignmentIdentityV1 {
    let (repository, artifact_ref) = context.artifact_context.as_ref().map_or_else(
        || {
            let repository = context.primary().map_or_else(
                || "<unknown>".to_string(),
                |repo| {
                    if repo.owner.is_empty() || repo.name.is_empty() {
                        repo.id
                            .split_once(':')
                            .map_or_else(|| repo.id.clone(), |(_, path)| path.to_string())
                    } else {
                        format!("{}/{}", repo.owner, repo.name)
                    }
                },
            );
            let artifact_ref = artifact_ref_from_work_item(context, &repository)
                .unwrap_or_else(|| context.work_item.target.clone());
            (repository, artifact_ref)
        },
        |bundle| {
            let artifact = &bundle.primary.artifact;
            let artifact_ref = match artifact.artifact_type {
                ArtifactType::Issue => {
                    format!("{}#{}", artifact.repository.path, artifact.number)
                }
                ArtifactType::PullRequest => {
                    format!("{} PR#{}", artifact.repository.path, artifact.number)
                }
            };
            (artifact.repository.path.clone(), artifact_ref)
        },
    );
    AgentAssignmentIdentityV1 {
        trace_context: context.trace_context.clone(),
        job_id: job_id.to_string(),
        repository,
        artifact_ref,
        role: context.work_item.role.clone(),
        action: context.action.clone(),
        correlation_key: context.correlation_key.clone(),
    }
}

fn artifact_ref_from_work_item(context: &WorkspaceContext, repository: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(&context.work_item.context).ok()?;
    let artifact = value.get("artifact")?;
    let number = artifact.get("number")?.as_u64()?;
    let suffix = match artifact.get("type").and_then(serde_json::Value::as_str) {
        Some("pull_request") => format!(" PR#{number}"),
        _ => format!("#{number}"),
    };
    Some(format!("{repository}{suffix}"))
}

pub(super) fn terminal_reserve_bytes(manifest: &TraceManifestV1) -> Result<u64, TraceError> {
    let base = |event| AgentRunEventV1 {
        version: temper_protocol_activity::ACTIVITY_PROTOCOL_VERSION,
        run_id: manifest.run_id.clone(),
        seq: u64::MAX,
        occurred_at: manifest.started_at.clone(),
        elapsed_ms: u64::MAX,
        assignment: manifest.assignment.clone(),
        agent_session_id: manifest.agent_session_id.clone(),
        scope: manifest.main_scope.clone(),
        turn: None,
        event,
    };
    let success = base(AgentActivityEventV1::RunFinished(RunFinishedV1 {
        status: RunStatusV1::Succeeded,
        duration_ms: u64::MAX,
        stop_reason: Some(StopReasonV1::Error),
    }));
    let mut bytes = serde_json::to_vec(&success)?.len();
    for code in [
        FailureCodeV1::Provider,
        FailureCodeV1::Timeout,
        FailureCodeV1::Tool,
        FailureCodeV1::ChildProcess,
        FailureCodeV1::Cancelled,
        FailureCodeV1::Policy,
        FailureCodeV1::Internal,
    ] {
        for class in [
            FailureClass::Transient,
            FailureClass::Permanent,
            FailureClass::Canceled,
            FailureClass::Protocol,
        ] {
            let failure = base(AgentActivityEventV1::RunFailed(RunFailedV1 {
                failure: FailureInfoV1 {
                    code,
                    message: host_failure_summary(class).to_string(),
                    retryable: class == FailureClass::Transient,
                },
            }));
            bytes = bytes.max(serde_json::to_vec(&failure)?.len());
        }
    }
    Ok(u64::try_from(bytes).unwrap_or(u64::MAX).saturating_add(1))
}

pub(super) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(super) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Privacy-safe terminal text selected exclusively from the trusted host
/// failure classification. Keep these static: provider/tool diagnostics and
/// child stderr must never become inputs to canonical terminal events.
pub(super) const fn host_failure_summary(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Transient => "agent run failed with a transient error",
        FailureClass::Permanent => "agent run failed with a permanent error",
        FailureClass::Canceled => "agent run was cancelled",
        FailureClass::Protocol => "agent run failed protocol validation",
    }
}
