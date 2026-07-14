// SPDX-License-Identifier: MPL-2.0

//! Resolution for durable agent-trace capture, storage, and query access.

use std::path::{Component, Path, PathBuf};

use temper_protocol_activity::AgentActivityCapturePolicyV1;

use crate::error::ConfigError;
use crate::resolve_options::ResolveOptions;
use crate::resolved::{AgentTraceSettings, ObservabilitySettings};
use crate::schema::{Config, Credentials};
use crate::secret_refs::{require_secret_payload, resolve_secret_reference};

const ENGINE_TRACE_JOURNAL_DIR: &str = "agent-traces/journal";
const WORKER_TRACE_SPOOL_DIR: &str = "agent-traces/worker-spool";

pub(crate) fn resolve_observability(
    config: &Config,
    credentials: &Credentials,
    state_dir: Option<&Path>,
    workspace_root: &Path,
    options: &ResolveOptions,
) -> Result<ObservabilitySettings, ConfigError> {
    let raw = &config.observability.agent_traces;
    let mut policy = AgentActivityCapturePolicyV1::default();
    if let Some(capture) = raw.capture {
        policy.capture = capture;
    }
    if let Some(retention_days) = raw.retention_days {
        policy.retention_days = retention_days;
    }
    if let Some(max_run_bytes) = raw.max_run_bytes {
        policy.max_run_bytes = max_run_bytes;
    }
    if let Some(capture_thinking) = raw.capture_thinking {
        policy.capture_thinking = capture_thinking;
    }

    if policy.max_run_bytes > i64::MAX as u64 {
        return Err(ConfigError::invalid(
            "observability.agent_traces.max_run_bytes exceeds the supported signed file-offset range",
        ));
    }
    policy.validate().map_err(|error| {
        ConfigError::invalid(
            error
                .to_string()
                .replace("capture_policy", "observability.agent_traces"),
        )
    })?;

    let resolved_read_token = resolve_secret_reference(
        "observability.agent_traces.read_token",
        raw.read_token.as_deref(),
        credentials,
        options.validate_secret_references,
    )?;
    let read_token_value = resolved_read_token
        .as_ref()
        .filter(|resolved| resolved.reference.available)
        .map(|resolved| require_secret_payload("observability.agent_traces.read_token", resolved))
        .transpose()?;
    let read_token = resolved_read_token.map(|resolved| resolved.reference);

    let (engine_journal_root, worker_spool_root) = match state_dir {
        Some(state_dir) => {
            let journal = state_dir.join(ENGINE_TRACE_JOURNAL_DIR);
            let spool = state_dir.join(WORKER_TRACE_SPOOL_DIR);
            ensure_trace_root_outside_workspace(&journal, workspace_root, "engine journal")?;
            ensure_trace_root_outside_workspace(&spool, workspace_root, "worker spool")?;
            (Some(journal), Some(spool))
        }
        None => (None, None),
    };

    Ok(ObservabilitySettings {
        agent_traces: AgentTraceSettings {
            policy,
            read_token,
            read_token_value,
            engine_journal_root,
            worker_spool_root,
        },
    })
}

fn ensure_trace_root_outside_workspace(
    trace_root: &Path,
    workspace_root: &Path,
    label: &str,
) -> Result<(), ConfigError> {
    let trace_root = lexical_normalize(trace_root);
    let workspace_root = lexical_normalize(workspace_root);
    if trace_root.starts_with(&workspace_root) || workspace_root.starts_with(&trace_root) {
        return Err(ConfigError::invalid(format!(
            "observability agent trace {label} root {} must be outside and separate from paths.workspace_dir {}",
            trace_root.display(),
            workspace_root.display()
        )));
    }
    Ok(())
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let absolute = path.is_absolute();
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                let last_is_parent = normalized
                    .file_name()
                    .is_some_and(|name| name == std::ffi::OsStr::new(".."));
                if last_is_parent || (!normalized.pop() && !absolute) {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}
