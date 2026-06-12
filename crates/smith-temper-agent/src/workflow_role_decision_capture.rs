//! Env-gated redacted capture files for workflow-role decisions.
//!
//! Captures are disabled unless `SMITH_WORKFLOW_ROLE_DECISION_CAPTURE_DIR` names
//! an existing writable directory. This module writes one bounded/redacted JSON
//! file per decision attempt and never turns a model decision into a workflow
//! failure.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use temper_process_protocol::{
    WORKFLOW_ROLE_DECISION_NO_ACTION, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};
use uuid::Uuid;

use crate::observability::{
    FIELD_PREVIEW_CHARS, REDACTED, contains_secret_like_text, redacted_preview,
};
use crate::provider::ProviderConfig;
use crate::workflow_role_decision::WorkflowRoleModelDecision;
use crate::workflow_role_decision_observability::WorkflowRoleTrace;

/// Environment variable that enables redacted workflow-role decision captures.
pub const WORKFLOW_ROLE_DECISION_CAPTURE_DIR_ENV: &str = "SMITH_WORKFLOW_ROLE_DECISION_CAPTURE_DIR";

const CAPTURE_SCHEMA_VERSION: u32 = 1;
const CAPTURE_PREVIEW_CHARS: usize = 1_000;
const FILE_ID_CHARS: usize = 96;
const LOCAL_ID_CHARS: usize = 36;

/// Disabled-by-default capture configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowRoleDecisionCapture {
    dir: Option<PathBuf>,
}

impl WorkflowRoleDecisionCapture {
    pub(crate) fn from_env() -> Self {
        Self::from_optional_dir(std::env::var_os(WORKFLOW_ROLE_DECISION_CAPTURE_DIR_ENV))
    }

    pub(crate) fn from_optional_dir(dir: Option<impl Into<PathBuf>>) -> Self {
        let dir = dir.and_then(|dir| {
            let dir = dir.into();
            if dir.as_os_str().is_empty() {
                None
            } else {
                Some(dir)
            }
        });
        Self { dir }
    }

    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self { dir: None }
    }

    #[cfg(test)]
    pub(crate) fn directory(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    pub(crate) fn write(&self, input: WorkflowRoleDecisionCaptureInput<'_>) -> CaptureWriteResult {
        self.write_with_local_id(
            input,
            current_unix_timestamp_ms(),
            &Uuid::new_v4().to_string(),
        )
    }

    fn write_with_local_id(
        &self,
        input: WorkflowRoleDecisionCaptureInput<'_>,
        timestamp_unix_ms: u64,
        local_id: &str,
    ) -> CaptureWriteResult {
        let Some(dir) = &self.dir else {
            return CaptureWriteResult::Disabled;
        };

        let record = DecisionCaptureFile::from_input(input, timestamp_unix_ms);
        let mut payload = match serde_json::to_vec_pretty(&record) {
            Ok(payload) => payload,
            Err(error) => {
                return CaptureWriteResult::Failed(CaptureWriteError::new(
                    "serialize",
                    error.to_string(),
                ));
            }
        };
        payload.push(b'\n');

        let primary = capture_file_path(dir, input.trace, local_id);
        match write_new_file(&primary, &payload) {
            Ok(()) => CaptureWriteResult::Written(primary),
            Err(error)
                if error.class == "already_exists" && !primary_stem_uses_local_id(input.trace) =>
            {
                let fallback = capture_file_path_with_local_suffix(dir, input.trace, local_id);
                match write_new_file(&fallback, &payload) {
                    Ok(()) => CaptureWriteResult::Written(fallback),
                    Err(error) => CaptureWriteResult::Failed(error),
                }
            }
            Err(error) => CaptureWriteResult::Failed(error),
        }
    }
}

/// Borrowed decision data used to build one capture file.
#[derive(Clone, Copy)]
pub(crate) struct WorkflowRoleDecisionCaptureInput<'a> {
    pub(crate) request: &'a WorkflowRoleDecisionRequest,
    pub(crate) trace: &'a WorkflowRoleTrace,
    pub(crate) provider: &'a ProviderConfig,
    pub(crate) system_prompt: Option<&'a str>,
    pub(crate) user_context: Option<&'a str>,
    pub(crate) model_decision: Option<&'a WorkflowRoleModelDecision>,
    pub(crate) final_reply: Option<&'a WorkflowRoleDecisionReply>,
    pub(crate) latency_ms: Option<u64>,
    pub(crate) outcome: &'static str,
    pub(crate) failure_class: Option<&'static str>,
}

/// Result of an attempted capture write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CaptureWriteResult {
    Disabled,
    Written(PathBuf),
    Failed(CaptureWriteError),
}

/// Bounded, non-payload capture write failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CaptureWriteError {
    class: &'static str,
    message: String,
}

impl CaptureWriteError {
    fn new(class: &'static str, message: impl Into<String>) -> Self {
        Self {
            class,
            message: redacted_preview(&message.into(), FIELD_PREVIEW_CHARS),
        }
    }

    pub(crate) fn class(&self) -> &'static str {
        self.class
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Serialize)]
struct DecisionCaptureFile {
    schema_version: u32,
    captured_at_unix_ms: u64,
    trace: CaptureTrace,
    workflow: CaptureWorkflow,
    provider: CaptureProvider,
    allowed_actions: Vec<String>,
    available_external_tool_ids: Vec<String>,
    prompt: Option<TextCapture>,
    context: Option<TextCapture>,
    model_decision: Option<ModelDecisionCapture>,
    final_reply: Option<ReplyCapture>,
    latency_ms: Option<u64>,
    outcome: &'static str,
    failure_class: Option<&'static str>,
}

impl DecisionCaptureFile {
    fn from_input(input: WorkflowRoleDecisionCaptureInput<'_>, captured_at_unix_ms: u64) -> Self {
        let identity = input.provider.observability_identity();
        Self {
            schema_version: CAPTURE_SCHEMA_VERSION,
            captured_at_unix_ms,
            trace: CaptureTrace::from_trace(input.trace),
            workflow: CaptureWorkflow::from_request(input.request, input.trace),
            provider: CaptureProvider {
                provider_id: bounded(identity.provider_id),
                model_id: bounded(identity.model_id),
                auth_mode: identity.auth_mode,
            },
            allowed_actions: allowed_action_names(input.request),
            available_external_tool_ids: external_tool_ids(input.request),
            prompt: input.system_prompt.map(TextCapture::from_text),
            context: input.user_context.map(TextCapture::from_text),
            model_decision: input
                .model_decision
                .map(ModelDecisionCapture::from_decision),
            final_reply: input.final_reply.map(ReplyCapture::from_reply),
            latency_ms: input.latency_ms,
            outcome: input.outcome,
            failure_class: input.failure_class,
        }
    }
}

#[derive(Serialize)]
struct CaptureTrace {
    run_id: Option<String>,
    tick_id: Option<String>,
    work_item_id: Option<String>,
    decision_id: Option<String>,
}

impl CaptureTrace {
    fn from_trace(trace: &WorkflowRoleTrace) -> Self {
        Self {
            run_id: trace.run_id.as_deref().map(bounded),
            tick_id: trace.tick_id.as_deref().map(bounded),
            work_item_id: trace.work_item_id.as_deref().map(bounded),
            decision_id: trace.decision_id.as_deref().map(bounded),
        }
    }
}

#[derive(Serialize)]
struct CaptureWorkflow {
    workflow_id: String,
    role_id: String,
    work_item_role: Option<String>,
    repository: Option<String>,
    queue: Option<String>,
    kind: Option<String>,
    artifact: CaptureArtifact,
}

impl CaptureWorkflow {
    fn from_request(request: &WorkflowRoleDecisionRequest, trace: &WorkflowRoleTrace) -> Self {
        Self {
            workflow_id: bounded(&request.workflow_id),
            role_id: bounded(request.role_manifest.id.as_str()),
            work_item_role: trace.role.as_deref().map(bounded),
            repository: trace.repository.as_deref().map(bounded),
            queue: trace.queue.as_deref().map(bounded),
            kind: trace.kind.as_deref().map(bounded),
            artifact: CaptureArtifact {
                artifact_type: trace.artifact_type.as_deref().map(bounded),
                number: trace.artifact_number.as_deref().map(bounded),
            },
        }
    }
}

#[derive(Serialize)]
struct CaptureArtifact {
    artifact_type: Option<String>,
    number: Option<String>,
}

#[derive(Serialize)]
struct CaptureProvider {
    provider_id: String,
    model_id: String,
    auth_mode: &'static str,
}

#[derive(Serialize)]
struct TextCapture {
    chars: usize,
    preview: String,
}

impl TextCapture {
    fn from_text(text: &str) -> Self {
        Self {
            chars: text.chars().count(),
            preview: redacted_preview(text, CAPTURE_PREVIEW_CHARS),
        }
    }
}

#[derive(Serialize)]
struct ModelDecisionCapture {
    action: String,
    reason_preview: String,
}

impl ModelDecisionCapture {
    fn from_decision(decision: &WorkflowRoleModelDecision) -> Self {
        Self {
            action: bounded(&decision.action),
            reason_preview: redacted_preview(&decision.reason, FIELD_PREVIEW_CHARS),
        }
    }
}

#[derive(Serialize)]
struct ReplyCapture {
    action: String,
    reason_preview: String,
}

impl ReplyCapture {
    fn from_reply(reply: &WorkflowRoleDecisionReply) -> Self {
        Self {
            action: bounded(&reply.action),
            reason_preview: redacted_preview(&reply.reason, FIELD_PREVIEW_CHARS),
        }
    }
}

fn bounded(value: &str) -> String {
    redacted_preview(value, FIELD_PREVIEW_CHARS)
}

fn allowed_action_names(request: &WorkflowRoleDecisionRequest) -> Vec<String> {
    std::iter::once(WORKFLOW_ROLE_DECISION_NO_ACTION.to_string())
        .chain(
            request
                .authorized_actions
                .iter()
                .map(|action| bounded(&action.action)),
        )
        .collect()
}

fn external_tool_ids(request: &WorkflowRoleDecisionRequest) -> Vec<String> {
    request
        .available_external_tools
        .iter()
        .map(|tool| bounded(tool.id.as_str()))
        .collect()
}

fn current_unix_timestamp_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn write_new_file(path: &Path, payload: &[u8]) -> Result<(), CaptureWriteError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| capture_io_error("open", error))?;
    file.write_all(payload)
        .map_err(|error| capture_io_error("write", error))
}

fn capture_io_error(operation: &'static str, error: std::io::Error) -> CaptureWriteError {
    let class = match error.kind() {
        ErrorKind::AlreadyExists => "already_exists",
        ErrorKind::NotFound => "not_found",
        ErrorKind::PermissionDenied => "permission_denied",
        _ => operation,
    };
    CaptureWriteError::new(class, error.to_string())
}

fn capture_file_path(dir: &Path, trace: &WorkflowRoleTrace, local_id: &str) -> PathBuf {
    dir.join(format!("{}.json", primary_file_stem(trace, local_id)))
}

fn capture_file_path_with_local_suffix(
    dir: &Path,
    trace: &WorkflowRoleTrace,
    local_id: &str,
) -> PathBuf {
    let stem = trace_file_stem(trace, local_id).unwrap_or_else(|| local_file_stem(local_id));
    dir.join(format!("{}-{}.json", stem, safe_local_id(local_id)))
}

fn primary_file_stem(trace: &WorkflowRoleTrace, local_id: &str) -> String {
    trace_file_stem(trace, local_id).unwrap_or_else(|| local_file_stem(local_id))
}

fn primary_stem_uses_local_id(trace: &WorkflowRoleTrace) -> bool {
    trace.decision_id.as_deref().is_none() && trace.work_item_id.as_deref().is_none()
}

fn trace_file_stem(trace: &WorkflowRoleTrace, local_id: &str) -> Option<String> {
    trace
        .decision_id
        .as_deref()
        .and_then(|id| path_safe_identifier(id, local_id))
        .map(|id| format!("decision-{id}"))
        .or_else(|| {
            trace
                .work_item_id
                .as_deref()
                .and_then(|id| path_safe_identifier(id, local_id))
                .map(|id| format!("work-item-{id}"))
        })
}

fn local_file_stem(local_id: &str) -> String {
    format!("decision-{}", safe_local_id(local_id))
}

fn safe_local_id(local_id: &str) -> String {
    path_safe_identifier(local_id, local_id).unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn path_safe_identifier(raw: &str, local_id: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || contains_secret_like_text(raw) || looks_like_sensitive_path(raw) {
        return None;
    }

    let mut sanitized = String::new();
    let mut last_separator = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            sanitized.push(ch);
            last_separator = false;
        } else if !last_separator {
            sanitized.push('-');
            last_separator = true;
        }
    }
    let sanitized = sanitized.trim_matches('-').to_string();
    if sanitized.is_empty() || sanitized == REDACTED {
        return None;
    }

    if sanitized.chars().count() <= FILE_ID_CHARS {
        return Some(sanitized);
    }

    let prefix = sanitized.chars().take(FILE_ID_CHARS).collect::<String>();
    Some(format!(
        "{}-{}",
        prefix.trim_matches('-'),
        safe_local_id_fragment(local_id)
    ))
}

fn safe_local_id_fragment(local_id: &str) -> String {
    let safe = local_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(LOCAL_ID_CHARS)
        .collect::<String>();
    if safe.is_empty() {
        "local".to_string()
    } else {
        safe
    }
}

fn looks_like_sensitive_path(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.starts_with('/')
        || lower.starts_with("~/")
        || lower.contains('\\')
        || lower.contains("auth.json")
        || lower.contains(".pi/agent")
        || lower.contains(".env")
        || lower.contains("api-key")
        || lower.contains("apikey")
        || lower.contains("credential")
}

#[cfg(test)]
#[path = "workflow_role_decision_capture_tests.rs"]
mod tests;
