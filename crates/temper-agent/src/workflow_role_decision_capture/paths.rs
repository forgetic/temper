//! Path-safe capture file naming.
//!
//! Capture file stems are derived from trace identifiers (decision / work-item
//! id) when those are present and safe, falling back to a random local id. Every
//! candidate is sanitized: secret-like or path-like identifiers are rejected so
//! no credential or filesystem path can leak into a file name.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::observability::{REDACTED, contains_secret_like_text};
use crate::workflow_role_decision_observability::WorkflowRoleTrace;

const FILE_ID_CHARS: usize = 96;
const LOCAL_ID_CHARS: usize = 36;

pub(super) fn capture_file_path(dir: &Path, trace: &WorkflowRoleTrace, local_id: &str) -> PathBuf {
    dir.join(format!("{}.json", primary_file_stem(trace, local_id)))
}

pub(super) fn capture_file_path_with_local_suffix(
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

pub(super) fn primary_stem_uses_local_id(trace: &WorkflowRoleTrace) -> bool {
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
