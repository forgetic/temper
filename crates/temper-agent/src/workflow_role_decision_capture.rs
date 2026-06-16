//! Env-gated redacted capture files for workflow-role decisions.
//!
//! Captures are disabled unless `ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR` names
//! an existing writable directory. This module writes one bounded/redacted JSON
//! file per decision attempt and never turns a model decision into a workflow
//! failure.
//!
//! The on-disk schema lives in [`record`]; path-safe file naming in [`paths`].
//! This file owns the enable/disable gate, the public input/result types, and
//! the atomic single-file write.

mod paths;
mod record;

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use temper_process_protocol::{WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest};
use uuid::Uuid;

use crate::observability::{FIELD_PREVIEW_CHARS, redacted_preview};
use crate::provider::ProviderConfig;
use crate::workflow_role_decision::WorkflowRoleModelDecision;
use crate::workflow_role_decision_observability::WorkflowRoleTrace;

use paths::{capture_file_path, capture_file_path_with_local_suffix, primary_stem_uses_local_id};
use record::DecisionCaptureFile;

/// Environment variable that enables redacted workflow-role decision captures.
/// The name lives here so the agent's `entry` / the CLI responder (the env
/// readers) and this module agree; nothing in this crate reads it.
pub const WORKFLOW_ROLE_DECISION_CAPTURE_DIR_ENV: &str = "ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR";

/// Disabled-by-default capture configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowRoleDecisionCapture {
    dir: Option<PathBuf>,
}

impl WorkflowRoleDecisionCapture {
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

#[cfg(test)]
#[path = "workflow_role_decision_capture_tests.rs"]
mod tests;
