//! The capability/role-aware coding-workspace agent run. Result parsing and
//! role-contract validation live in [`super::result`].

use std::path::Path;

use crate::codebase_memory::{
    CodebaseMemoryToolMetadata, CodebaseMemoryToolset, build_codebase_memory_toolset,
};
use crate::prompt_overlays::PromptOverlays;
use crate::provider::ProviderConfig;
use crate::usage::RunTotals;
use temper_protocol_agent::{AgentToolConfig, WorkspaceContext, WorkspaceResult};

use super::result::{collect_text, parse_result, validate_contract, validate_verdict_vocabulary};
use super::tools::{SUBAGENT_GUIDANCE, add_subagents, tool_registry_for_context};
use super::{
    Capability, CodingAgentError, SubmitForPrCallback, SubmitForPrHost, bind_submit_for_pr_host,
    default_submit_for_pr_host, system_prompt, user_context,
};

struct PreparedCodebaseMemoryTools {
    prompt_section: Option<String>,
    toolset: CodebaseMemoryToolset,
}

async fn prepare_codebase_memory_tools(
    tool_config: Option<&AgentToolConfig>,
    role: &str,
) -> Result<PreparedCodebaseMemoryTools, CodingAgentError> {
    let toolset = build_codebase_memory_toolset(tool_config, role)
        .await
        .map_err(|error| CodingAgentError::CodebaseMemory(error.to_string()))?;
    let prompt_section = codebase_memory_prompt_section(toolset.registered_tool_metadata());
    Ok(PreparedCodebaseMemoryTools {
        prompt_section,
        toolset,
    })
}

pub(crate) fn codebase_memory_prompt_section(
    tools: &[CodebaseMemoryToolMetadata],
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let mut tools = tools.to_vec();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    let rendered_tools = tools
        .iter()
        .map(|tool| {
            let summary = tool.description.lines().next().unwrap_or_default().trim();
            if summary.is_empty() {
                format!("- `{}`", tool.name)
            } else {
                format!("- `{}`: {summary}", tool.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "\nCODEBASE MEMORY:\n\
         You have repository-index tools for architecture, symbol search, code search,\n\
         and call/impact tracing.\n\n\
         Use them early for non-trivial tasks:\n\
         - architect: map affected areas before triage/breakdown;\n\
         - engineer: find relevant symbols/callers before editing;\n\
         - reviewer: inspect impacted code paths and callers before verdicts.\n\n\
         Treat the graph as an index, not truth. Verify exact code with read/grep/git diff\n\
         before editing or making final claims.\n\n\
         Registered codebase-memory tools:\n{rendered_tools}\n"
    ))
}

/// Runs one capability/role-aware coding-workspace turn on anvil's native
/// sans-IO agent loop ([`temper_agent_core::run_sub_agent`]).
///
/// Builds the role prompt + overlays, the role's tongs tools scoped to `cwd`,
/// and per-request stream options; runs the deterministic
/// [`temper_agent_core::AgentMachine`] driven by a skein shell; parses the model's
/// final JSON into a [`WorkspaceResult`]; and validates the role contract (an
/// engineer head path must leave a product diff or route a verdict).
///
/// `config_dir` is the resolved operator config dir (default
/// `$XDG_CONFIG_HOME/anvil` else `~/.config/anvil`, overridable via
/// `--config-dir` / `ANVIL_CONFIG_DIR`). When present, per-role operator
/// prompt overlays from it and the checkout's root `AGENTS.md` are layered
/// onto the built-in role prompt as clearly-delimited context. Missing
/// dir/files are a clean no-op. See [`crate::prompt_overlays`].
///
/// Must be awaited inside a skein engine task (the sub-agent's drive loop
/// reads the runtime clock and its shell spawns I/O).
pub async fn run_coding_agent_native(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
) -> Result<WorkspaceResult, CodingAgentError> {
    run_coding_agent_native_with_options(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        false,
    )
    .await
}

/// [`run_coding_agent_native`] with parsed non-secret agent tool config.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_tool_config(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    tool_config: Option<&AgentToolConfig>,
) -> Result<WorkspaceResult, CodingAgentError> {
    let (result, _totals) = run_coding_agent_native_with_totals_tool_config_and_submit_for_pr(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        false,
        tool_config,
        Some(default_submit_for_pr_host()),
    )
    .await?;
    Ok(result)
}

/// [`run_coding_agent_native`] with an explicit host submit callback.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_submit_for_pr(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    submit_for_pr: Option<SubmitForPrHost>,
) -> Result<WorkspaceResult, CodingAgentError> {
    let (result, _totals) = run_coding_agent_native_with_totals_and_submit_for_pr(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        false,
        submit_for_pr,
    )
    .await?;
    Ok(result)
}

/// [`run_coding_agent_native`] with optional features. When `enable_subagents`
/// is set, the role agent is given an `investigate` tool that delegates a
/// read-only investigation to a nested sub-agent scoped to the same checkout
/// (the parent can fan out several at once since the tool is read-only /
/// parallel-safe). Default coding behavior is unchanged when it is off.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_options(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
) -> Result<WorkspaceResult, CodingAgentError> {
    let (result, _totals) = run_coding_agent_native_with_totals(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
    )
    .await?;
    Ok(result)
}

/// [`run_coding_agent_native_with_options`] with an explicit host submit
/// callback.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_options_and_submit_for_pr(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    submit_for_pr: Option<SubmitForPrHost>,
) -> Result<WorkspaceResult, CodingAgentError> {
    run_coding_agent_native_with_options_tool_config_and_submit_for_pr(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        None,
        submit_for_pr,
    )
    .await
}

/// [`run_coding_agent_native_with_options`] with parsed non-secret agent tool
/// config and an explicit host submit callback.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_options_tool_config_and_submit_for_pr(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    tool_config: Option<&AgentToolConfig>,
    submit_for_pr: Option<SubmitForPrHost>,
) -> Result<WorkspaceResult, CodingAgentError> {
    let (result, _totals) = run_coding_agent_native_with_totals_tool_config_and_submit_for_pr(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        tool_config,
        submit_for_pr,
    )
    .await?;
    Ok(result)
}

/// [`run_coding_agent_native_with_options`] while also returning the run's
/// [`RunTotals`] (input/output tokens + tool-call count, summed across the main
/// run and every nested sub-agent). The standalone runner folds these into the
/// §7 `agent.finished` info line; callers that don't need them use the
/// totals-discarding wrappers above.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_totals(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
) -> Result<(WorkspaceResult, RunTotals), CodingAgentError> {
    run_coding_agent_native_with_totals_and_submit_for_pr(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        Some(default_submit_for_pr_host()),
    )
    .await
}

/// [`run_coding_agent_native_with_totals`] with an explicit host submit
/// callback. Passing `None` disables `submit_for_pr` for this run; callers use
/// this for an out-of-process agent session that did not receive a worker-owned
/// side channel.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_totals_and_submit_for_pr(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    submit_for_pr: Option<SubmitForPrHost>,
) -> Result<(WorkspaceResult, RunTotals), CodingAgentError> {
    run_coding_agent_native_with_totals_tool_config_and_submit_for_pr(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        None,
        submit_for_pr,
    )
    .await
}

/// [`run_coding_agent_native_with_totals`] with parsed non-secret agent tool
/// config and an explicit host submit callback. Passing `None` for
/// `submit_for_pr` disables the submit tool for this run.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_totals_tool_config_and_submit_for_pr(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    tool_config: Option<&AgentToolConfig>,
    submit_for_pr: Option<SubmitForPrHost>,
) -> Result<(WorkspaceResult, RunTotals), CodingAgentError> {
    let capability = Capability::for_role(&context.work_item.role);
    let codebase_memory =
        prepare_codebase_memory_tools(tool_config, &context.work_item.role).await?;
    let provider = provider_config.build_provider()?;

    let mut role_prompt = system_prompt(capability, &context.allowed_verdicts);
    if let Some(section) = &codebase_memory.prompt_section {
        role_prompt.push_str(section);
    }
    if enable_subagents {
        role_prompt.push_str(SUBAGENT_GUIDANCE);
    }
    let user = user_context(context);
    let overlays = PromptOverlays::load(config_dir, cwd, capability);
    let turns = overlays.compose_turns(
        &role_prompt,
        &user,
        provider_config.required_system_identity(),
    );

    // Same per-request stream options the pi path sets.
    let stream_options = tongs::provider::StreamOptions {
        api_key: Some(provider_config.resolve_bearer().await?),
        temperature: provider_config.temperature(),
        thinking_level: provider_config.coding_thinking_level(),
        headers: provider_config.request_headers_for_session(context.agent_session.as_ref()),
        ..tongs::provider::StreamOptions::default()
    };

    // Token accounting: one shared totals ledger across the main run and all
    // nested sub-agent runs; per-turn/tool lines plus an end-of-run summary
    // go to stderr (stdout is the protocol stream).
    let totals = std::sync::Arc::new(crate::usage::UsageTotals::default());
    let events: std::sync::Arc<dyn temper_agent_core::EventSink> = std::sync::Arc::new(
        crate::usage::UsageLogger::new(crate::usage::MAIN_SCOPE, std::sync::Arc::clone(&totals)),
    );

    let submit_for_pr: Option<SubmitForPrCallback> = submit_for_pr
        .filter(|_| super::submit_for_pr_available(context))
        .map(|host| bind_submit_for_pr_host(host, context, cwd));
    let mut tools = tool_registry_for_context(capability, context, cwd, submit_for_pr);
    codebase_memory.toolset.append_to_registry(&mut tools);
    if enable_subagents {
        tools = add_subagents(
            handle.clone(),
            tools,
            provider_config,
            &stream_options,
            cwd,
            &totals,
        );
    }

    let sub_agent = temper_agent_core::SubAgent {
        system_prompt: Some(turns.system),
        user_message: turns.user,
        tools,
        max_iterations,
        provider,
        stream_options,
    };
    // Salient-arg preview for ToolStart human lines: computed in the pure core
    // (where the raw call args live) via this shell-supplied closure, which owns
    // the workspace `cwd` for repo-relative paths (agent-log-cleanup plan, B/D).
    let arg_preview = crate::usage::tool_arg_preview_hook(cwd.to_path_buf());
    let model_id = provider_config.model_id().to_string();
    let outcome = async {
        let (_control, run) = temper_agent_core::run_sub_agent_controllable_with_hooks(
            handle.clone(),
            sub_agent,
            events,
            None,
            Some(arg_preview),
        )?;
        run.await
    }
    .await
    .map_err(|error| classify_run_error(&model_id, error.to_string()))?;
    totals.emit_summary();
    let run_totals = totals.snapshot();

    if matches!(outcome.stop, temper_agent_core::AgentStop::ModelError) {
        let reason = outcome
            .final_message
            .error_message
            .clone()
            .unwrap_or_else(|| "provider reported an error stop".to_string());
        return Err(classify_run_error(&model_id, reason));
    }

    let text = collect_text(&outcome.final_message.content);
    let result = parse_result(&text).inspect_err(|_err| {
        tracing::warn!(
            target: "temper::agent",
            "agent final message contained no parseable WorkspaceResult envelope; \
             first 200 chars: {}",
            &text.chars().take(200).collect::<String>()
        );
    })?;
    validate_verdict_vocabulary(&result, &context.allowed_verdicts)?;
    validate_contract(capability, &result, cwd, context)?;
    Ok((result, run_totals))
}

/// Classifies a run/stop error message, promoting a model-availability
/// rejection to [`CodingAgentError::ModelUnavailable`] (which names the model
/// and points at the override env vars) and leaving everything else as a
/// generic abnormal stop.
///
/// Providers phrase this differently — Anthropic returns `404` with
/// `"<Model> is not available"` / `"Please use Opus 4.8"`; OpenAI returns
/// `"model ... does not exist or you do not have access"`. We match on these
/// stable fragments rather than a status code because the message reaches us
/// as flattened text.
pub(crate) fn classify_run_error(model: &str, message: String) -> CodingAgentError {
    let lower = message.to_ascii_lowercase();
    let unavailable = lower.contains("is not available")
        || lower.contains("does not exist")
        || lower.contains("do not have access")
        || lower.contains("model_not_found")
        || (lower.contains("model") && lower.contains("unavailable"));
    if unavailable {
        CodingAgentError::ModelUnavailable {
            model: model.to_string(),
            detail: message,
        }
    } else {
        CodingAgentError::AgentStopped(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    };
    use tongs::tools::ToolEffects;

    fn fake_server_script() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("fake_codebase_memory_mcp.py"),
            r#"
import json
import sys

TOOLS = [
    {"name": "search_code", "description": "Search indexed code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
    {"name": "delete_project", "description": "Delete project", "inputSchema": {"type": "object", "properties": {}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "fake-codebase-memory", "version": "1"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": "fake result"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
        )
        .expect("write fake server");
        dir
    }

    fn script_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("fake_codebase_memory_mcp.py")
    }

    fn config(
        dir: &tempfile::TempDir,
        mode: CodebaseMemoryMode,
        roles: Vec<&str>,
    ) -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode,
                command: "python3".to_string(),
                args: vec!["-u".to_string(), script_path(dir).display().to_string()],
                roles: roles.into_iter().map(str::to_string).collect(),
                index: CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 2,
            }),
        }
    }

    fn bad_command_config(mode: CodebaseMemoryMode) -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode,
                command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
                args: Vec::new(),
                roles: vec!["engineer".to_string()],
                index: CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 1,
            }),
        }
    }

    #[test]
    fn codebase_memory_prompt_appears_only_after_safe_tools_registered() {
        let dir = fake_server_script();
        temper_agent_io::block_on(async move {
            let absent = prepare_codebase_memory_tools(None, "engineer")
                .await
                .expect("absent config is ok");
            assert!(absent.prompt_section.is_none());
            assert!(absent.toolset.registered_tool_names().is_empty());

            let role_mismatch = config(&dir, CodebaseMemoryMode::Required, vec!["reviewer"]);
            let mismatch = prepare_codebase_memory_tools(Some(&role_mismatch), "engineer")
                .await
                .expect("role mismatch is ok");
            assert!(mismatch.prompt_section.is_none());
            assert!(mismatch.toolset.registered_tool_names().is_empty());

            let auto_unavailable = bad_command_config(CodebaseMemoryMode::Auto);
            let unavailable = prepare_codebase_memory_tools(Some(&auto_unavailable), "engineer")
                .await
                .expect("auto startup failure is best effort");
            assert!(unavailable.prompt_section.is_none());
            assert!(unavailable.toolset.registered_tool_names().is_empty());

            let enabled = config(&dir, CodebaseMemoryMode::Required, vec!["engineer"]);
            let prepared = prepare_codebase_memory_tools(Some(&enabled), "engineer")
                .await
                .expect("required fake server starts");
            let prompt = prepared
                .prompt_section
                .as_deref()
                .expect("registered tools produce prompt section");
            assert!(prompt.contains("CODEBASE MEMORY"));
            assert!(prompt.contains("codebase_memory_search_code"));
            assert!(prompt.contains("Search indexed code"));
            assert!(!prompt.contains("codebase_memory_delete_project"));

            let mut registry = tongs::tools::ToolRegistry::new();
            prepared.toolset.append_to_registry(&mut registry);
            let tool = registry
                .get("codebase_memory_search_code")
                .expect("safe tool registered");
            assert_eq!(tool.effects(), ToolEffects::read());
        });
    }

    #[test]
    fn required_codebase_memory_startup_failure_is_coding_agent_error() {
        let required = bad_command_config(CodebaseMemoryMode::Required);
        temper_agent_io::block_on(async move {
            let error = match prepare_codebase_memory_tools(Some(&required), "engineer").await {
                Ok(_) => panic!("required mode startup failure must fail the run"),
                Err(error) => error,
            };
            match error {
                CodingAgentError::CodebaseMemory(message) => {
                    assert!(message.contains("required codebase-memory MCP startup failed"));
                    assert!(message.contains("spawn MCP command"));
                }
                other => panic!("expected codebase-memory setup error, got {other:?}"),
            }
        });
    }
}
