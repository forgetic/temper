// SPDX-License-Identifier: MPL-2.0

//! The worker ↔ agent protocol driver: drive the native coding loop in cwd
//! and write the result.
//!
//! Every input — the [`AgentConfig`], the [`WorkspaceContext`], the cwd, and the
//! result-file path — is supplied by [`crate::entry`], the single env-reading
//! module. Nothing here touches `std::env`.

use std::path::{Path, PathBuf};

use temper_agent::{
    CodingAgentError, run_coding_agent_native_with_options_tool_config_and_submit_for_pr,
};
use temper_protocol_agent::{WorkspaceContext, WorkspaceResult};

use crate::config::AgentConfig;

/// Drives one protocol run from fully-resolved inputs.
///
/// `config` carries the provider wiring + the host-derived session knobs;
/// `context` is the already-parsed workspace context; `cwd` is the prepared
/// checkout the worker handed us; `result_path` is the file the result is written
/// to. All four are resolved (and the env read) in [`crate::entry`].
pub(crate) fn drive(
    config: AgentConfig,
    context: WorkspaceContext,
    cwd: PathBuf,
    result_path: String,
) -> Result<(), String> {
    let result =
        drive_coding_loop(&config, &context, &cwd).map_err(|error| describe_agent_error(&error))?;

    write_result(&result_path, &result)
}

/// Runs the native coding loop on the async runtime.
///
/// Takes the session's per-subsystem [`AgentConfig`]: the provider, iteration
/// cap, config dir, and sub-agent toggle all come from it.
fn drive_coding_loop(
    config: &AgentConfig,
    context: &WorkspaceContext,
    cwd: &Path,
) -> Result<WorkspaceResult, CodingAgentError> {
    // Clone the values the run consumes so the originals survive the terminal
    // result write; the closure moves only these clones (and the owned config
    // knobs), so it satisfies the `'static` bound `block_on_with` requires.
    let run_context = context.clone();
    let run_cwd = cwd.to_path_buf();
    let provider = config.provider.clone();
    let max_iterations = config.max_iterations;
    let config_dir = config.config_dir.clone();
    let enable_subagents = config.enable_subagents;
    let tool_config = config.tool_config.clone();
    let submit_for_pr = config.submit_for_pr.clone();
    temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_options_tool_config_and_submit_for_pr(
            handle,
            &provider,
            &run_context,
            &run_cwd,
            max_iterations,
            config_dir.as_deref(),
            enable_subagents,
            tool_config.as_ref(),
            submit_for_pr,
        )
        .await
    })
}

fn write_result(result_path: &str, result: &WorkspaceResult) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(result).map_err(|error| format!("serialize result: {error}"))?;
    std::fs::write(result_path, bytes)
        .map_err(|error| format!("write result file {result_path}: {error}"))
}

/// Renders a coding-agent error for stderr. The worker re-derives the
/// transient/permanent class from the process exit (non-zero ⇒ transient) plus
/// a missing result file (⇒ permanent); the message here is for humans.
///
/// A model-unavailability rejection (a suspended model alias, or a tier that
/// does not grant the configured model) is flagged with an explicit
/// `model-unavailable:` prefix so it stands out in logs as a configuration
/// problem the `Display` text already explains how to fix — not a transient
/// network blip to be retried blindly.
fn describe_agent_error(error: &CodingAgentError) -> String {
    match error {
        CodingAgentError::ModelUnavailable { .. } => {
            format!("model-unavailable: {error}")
        }
        _ => format!("coding agent failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use temper_agent::ProviderConfig;
    use temper_protocol_agent::{
        AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
        WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
    };

    #[test]
    fn drive_passes_tool_config_to_native_loop() {
        let temp = tempfile::tempdir().expect("tempdir");
        let result_path = temp.path().join("result.json");
        let config = AgentConfig::new(
            ProviderConfig::new(
                "test-provider",
                "test-model",
                "https://llm.example",
                "test-key",
            ),
            1,
            false,
            None,
        )
        .with_tool_config(Some(required_bad_tool_config()));

        let error = drive(
            config,
            workspace_context("engineer"),
            temp.path().to_path_buf(),
            result_path.display().to_string(),
        )
        .expect_err("required codebase-memory startup failure aborts session");

        assert!(error.contains("codebase-memory tool setup failed"));
        assert!(error.contains("required codebase-memory MCP startup failed"));
        assert!(!result_path.exists());
    }

    fn required_bad_tool_config() -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode: CodebaseMemoryMode::Required,
                command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
                args: Vec::new(),
                roles: vec!["engineer".to_string()],
                index: CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 1,
            }),
        }
    }

    fn workspace_context(role: &str) -> WorkspaceContext {
        WorkspaceContext {
            repos: vec![WorkspaceRepository {
                id: "repo-1".to_string(),
                owner: "acme".to_string(),
                name: "demo".to_string(),
                default_branch: "main".to_string(),
                dir: ".".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/pr-for-code-1".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: role.to_string(),
                queue: "code_ready".to_string(),
                kind: "code".to_string(),
                target: "Issue { number: ItemNumber(1) }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-1".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: vec!["needs_architect".to_string()],
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: WorkspaceGuidance::default(),
            pull_request_freshness: None,
            agent_session: None,
        }
    }
}
