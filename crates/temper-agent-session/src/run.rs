// SPDX-License-Identifier: MPL-2.0

//! The worker ↔ agent protocol driver: drive the native coding loop in cwd
//! and write the result.
//!
//! Every input — the [`AgentConfig`], the [`WorkspaceContext`], the cwd, and the
//! result-file path — is supplied by [`crate::entry`], the single env-reading
//! module. Nothing here touches `std::env`.

use std::path::{Path, PathBuf};

use temper_agent::{
    AgentActivityConfig, AgentContainmentContext, CodingAgentError, ContainmentScope,
    protocol_model_failure, run_coding_agent_native_with_totals_tool_config_hosts_and_containment,
};
use temper_protocol_agent::{AgentTerminalOutputV1, WorkspaceContext, WorkspaceResult};

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
    terminal_output_path: Option<String>,
) -> Result<(), String> {
    match drive_coding_loop(&config, &context, &cwd) {
        Ok(result) => write_result(&result_path, &result),
        Err(error) => {
            if let Some(path) = terminal_output_path.as_deref() {
                write_terminal_failure(path, &error)?;
            }
            Err(describe_agent_error(&error))
        }
    }
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
    let runtime_limits = config.runtime_limits;
    let submit_for_pr = config.submit_for_pr.clone();
    let forge_context = config.forge_context.clone();
    let activity_config = AgentActivityConfig {
        policy: config.trace_policy.clone(),
        address: config.activity_address.clone(),
        lifecycle_address: config.lifecycle_address.clone(),
        lifecycle_reporter: None,
        cancellation: Default::default(),
        operator_transcript: config.operator_transcript.clone(),
    };
    // The out-of-process supervisor passes its delegated job cgroup as an
    // inherited descriptor. The production factory discovers that descriptor;
    // the typed outer scope records that every nested owner is below it.
    let containment = AgentContainmentContext::production(Some(ContainmentScope::Job));
    temper_agent_io::block_on_with(move |_cx, handle| async move {
        let (result, _totals) =
            run_coding_agent_native_with_totals_tool_config_hosts_and_containment(
                handle,
                &provider,
                &run_context,
                &run_cwd,
                max_iterations,
                config_dir.as_deref(),
                enable_subagents,
                tool_config.as_ref(),
                submit_for_pr,
                forge_context,
                activity_config,
                runtime_limits,
                containment,
            )
            .await?;
        Ok(result)
    })
}

fn write_result(result_path: &str, result: &WorkspaceResult) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(result).map_err(|error| format!("serialize result: {error}"))?;
    std::fs::write(result_path, bytes)
        .map_err(|error| format!("write result file {result_path}: {error}"))
}

/// Writes only the closed, bounded first-party terminal carrier. Non-model
/// failures intentionally leave no carrier for the worker to consume.
fn write_terminal_failure(path: &str, error: &CodingAgentError) -> Result<(), String> {
    let diagnostic = match error {
        CodingAgentError::ModelFailure(diagnostic)
        | CodingAgentError::ModelUnavailable { diagnostic, .. } => diagnostic,
        _ => return Ok(()),
    };
    let output = AgentTerminalOutputV1::model_failure(protocol_model_failure(diagnostic.as_ref()));
    output
        .validate()
        .map_err(|error| format!("validate terminal model failure: {error}"))?;
    let bytes = serde_json::to_vec_pretty(&output)
        .map_err(|error| format!("serialize terminal model failure: {error}"))?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("write terminal output file {path}: {error}"))
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
    fn typed_stop_diagnostics_keep_stable_reason_tokens() {
        let budget =
            describe_agent_error(&CodingAgentError::BudgetExhausted { max_iterations: 17 });
        assert!(budget.contains("budget_exhausted"));

        for authority in [
            temper_agent::AgentAbortAuthority::WorkerRequested,
            temper_agent::AgentAbortAuthority::Unrequested,
        ] {
            let aborted = describe_agent_error(&CodingAgentError::Aborted { authority });
            assert!(aborted.contains("aborted"));
            assert!(aborted.contains(&authority.to_string()));
        }
    }

    #[test]
    fn first_party_model_error_writes_only_bounded_terminal_diagnostic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("terminal.json");
        let error = CodingAgentError::ModelFailure(Box::new(
            temper_agent::ModelFailureDiagnostic::redacted_unknown(
                "openai-codex",
                "gpt-test",
                false,
            ),
        ));

        write_terminal_failure(path.to_str().unwrap(), &error).expect("terminal output writes");

        let output: AgentTerminalOutputV1 =
            serde_json::from_slice(&std::fs::read(path).expect("terminal output is readable"))
                .expect("terminal output parses");
        output.validate().expect("terminal output validates");
        assert_eq!(output.model_failure.provider, "openai-codex");
        assert_eq!(output.model_failure.model, "gpt-test");
        assert!(!output.model_failure.retryable);
        let wire = serde_json::to_string(&output).unwrap();
        for forbidden in ["prompt", "raw_response", "credentials", "stderr"] {
            assert!(
                !wire.contains(forbidden),
                "terminal carrier leaked {forbidden}"
            );
        }
    }

    #[test]
    fn non_model_error_does_not_create_terminal_carrier() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("terminal.json");
        write_terminal_failure(
            path.to_str().unwrap(),
            &CodingAgentError::BudgetExhausted { max_iterations: 7 },
        )
        .expect("non-model failure is ignored");
        assert!(!path.exists());
    }

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
            None,
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
                retention: Default::default(),
            }),
        }
    }

    fn workspace_context(role: &str) -> WorkspaceContext {
        WorkspaceContext {
            trace_context: None,
            artifact_context: None,
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
