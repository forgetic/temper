//! The capability/role-aware coding-workspace agent run. Result parsing and
//! role-contract validation live in [`super::result`].

use std::path::Path;

use crate::prompt_overlays::PromptOverlays;
use crate::provider::ProviderConfig;
use temper_agent_protocol::{WorkspaceContext, WorkspaceResult};

use super::result::{collect_text, parse_result, validate_contract, validate_verdict_vocabulary};
use super::tools::{
    CHECKPOINT_GUIDANCE, CheckpointTool, SUBAGENT_GUIDANCE, add_subagents, tool_registry,
};
use super::{Capability, CheckpointHook, CodingAgentError, system_prompt, user_context};

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
    run_coding_agent_native_with_hooks(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        None,
        None,
        None,
    )
    .await
}

/// [`run_coding_agent_native_with_options`] with the phase-6b hooks: an
/// optional resume note appended to the user turn (a re-dispatched agent
/// continuing a checkpointed branch), an optional [`temper_agent_core::TurnHook`]
/// awaited before each model call (the safety-backstop checkpointer), and an
/// optional [`CheckpointHook`] backing the model-driven `checkpoint` tool.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_hooks(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    resume_note: Option<&str>,
    turn_hook: Option<std::sync::Arc<dyn temper_agent_core::TurnHook>>,
    checkpoint_hook: Option<std::sync::Arc<dyn CheckpointHook>>,
) -> Result<WorkspaceResult, CodingAgentError> {
    let capability = Capability::for_role(&context.work_item.role);
    let provider = provider_config.build_provider()?;

    let mut role_prompt = system_prompt(capability, &context.allowed_verdicts);
    if enable_subagents {
        role_prompt.push_str(SUBAGENT_GUIDANCE);
    }
    if checkpoint_hook.is_some() {
        role_prompt.push_str(CHECKPOINT_GUIDANCE);
    }
    let mut user = user_context(context);
    if let Some(note) = resume_note {
        user.push_str("\n\n## Resume\n");
        user.push_str(note);
    }
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
        headers: provider_config.request_headers(),
        ..tongs::provider::StreamOptions::default()
    };

    // Token accounting: one shared totals ledger across the main run and all
    // nested sub-agent runs; per-turn/tool lines plus an end-of-run summary
    // go to stderr (stdout is the protocol stream).
    let totals = std::sync::Arc::new(crate::usage::UsageTotals::default());
    let events: std::sync::Arc<dyn temper_agent_core::EventSink> = std::sync::Arc::new(
        crate::usage::UsageLogger::new(crate::usage::MAIN_SCOPE, std::sync::Arc::clone(&totals)),
    );

    let mut tools = tool_registry(capability, cwd);
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
    if let Some(hook) = &checkpoint_hook {
        tools.push(Box::new(CheckpointTool { hook: hook.clone() }));
    }

    let sub_agent = temper_agent_core::SubAgent {
        system_prompt: Some(turns.system),
        user_message: turns.user,
        tools,
        max_iterations,
        provider,
        stream_options,
    };
    let model_id = provider_config.model_id().to_string();
    let outcome = async {
        let (_control, run) = temper_agent_core::run_sub_agent_controllable_with_hook(
            handle.clone(),
            sub_agent,
            events,
            turn_hook,
        )?;
        run.await
    }
    .await
    .map_err(|error| classify_run_error(&model_id, error.to_string()))?;
    totals.emit_summary();

    if matches!(outcome.stop, temper_agent_core::AgentStop::ModelError) {
        let reason = outcome
            .final_message
            .error_message
            .clone()
            .unwrap_or_else(|| "provider reported an error stop".to_string());
        return Err(classify_run_error(&model_id, reason));
    }

    let text = collect_text(&outcome.final_message.content);
    let result = parse_result(&text).map_err(|err| {
        tracing::warn!(
            target: "temper_agent",
            "agent final message contained no parseable WorkspaceResult envelope; \
             first 200 chars: {}",
            &text.chars().take(200).collect::<String>()
        );
        err
    })?;
    validate_verdict_vocabulary(&result, &context.allowed_verdicts)?;
    validate_contract(capability, &result, cwd, context)?;
    Ok(result)
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
