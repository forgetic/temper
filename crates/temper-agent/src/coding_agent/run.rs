//! The capability/role-aware coding-workspace agent run. Result parsing and
//! role-contract validation live in [`super::result`].

use std::path::Path;

use crate::prompt_overlays::PromptOverlays;
use crate::provider::ProviderConfig;
use crate::usage::RunTotals;
use temper_protocol_agent::{
    AgentRuntimeLimitsV1, AgentToolConfig, WorkspaceContext, WorkspaceResult,
};

use super::codebase_memory::prepare_codebase_memory_tools;
use super::prompt::{system_prompt_with_registry, user_context_with_registry};
use super::result::{
    collect_text, parse_result, validate_contract, validate_verdict_contract,
    validate_verdict_vocabulary,
};
use super::tools::{add_subagents, tool_registry_for_context};
use super::{
    Capability, CodingAgentError, ForgeContextHost, SubmitForPrCallback, SubmitForPrHost,
    bind_submit_for_pr_host, default_submit_for_pr_host,
};

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
    run_coding_agent_native_with_totals_tool_config_and_hosts(
        handle,
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        tool_config,
        submit_for_pr,
        None,
        crate::activity::AgentActivityConfig::default(),
        AgentRuntimeLimitsV1::default(),
    )
    .await
}

/// Full host-controlled run surface. Both hosts are bound to this run by the
/// caller; absence keeps the corresponding model-visible tools disabled.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_totals_tool_config_and_hosts(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    tool_config: Option<&AgentToolConfig>,
    submit_for_pr: Option<SubmitForPrHost>,
    forge_context: Option<ForgeContextHost>,
    activity_config: crate::activity::AgentActivityConfig,
    runtime_limits: AgentRuntimeLimitsV1,
) -> Result<(WorkspaceResult, RunTotals), CodingAgentError> {
    let operation_limits = temper_agent_core::AgentOperationLimits {
        tool_timeout: std::time::Duration::from_secs(runtime_limits.tool_timeout_secs),
        model_connect_timeout: std::time::Duration::from_secs(
            runtime_limits.model_connect_timeout_secs,
        ),
        model_idle_timeout: std::time::Duration::from_secs(runtime_limits.model_idle_timeout_secs),
    };
    let capability = Capability::for_role(&context.work_item.role);
    let codebase_memory =
        prepare_codebase_memory_tools(tool_config, &context.work_item.role, context, cwd).await?;
    let model_identity = temper_agent_core::ModelIdentity::new(
        provider_config.provider_id(),
        provider_config.model_id(),
    );
    let provider = provider_config.build_provider()?;

    // Same per-request stream options the pi path sets.
    let stream_options = tongs::provider::StreamOptions {
        api_key: Some(provider_config.resolve_bearer().await?),
        temperature: provider_config.temperature(),
        thinking_level: provider_config.coding_thinking_level(),
        headers: provider_config.request_headers_for_session(context.agent_session.as_ref()),
        ..tongs::provider::StreamOptions::default()
    };

    // One scope factory feeds the optional activity projections and installs a
    // separate correctness-lifecycle sink. Lifecycle never passes through the
    // capture policy, trace queue, or storage projection.
    let totals = std::sync::Arc::new(crate::usage::UsageTotals::default());
    let scope_factory =
        crate::activity::ScopeFactory::new(activity_config, std::sync::Arc::clone(&totals));
    let main_observability = scope_factory.main(crate::usage::MAIN_SCOPE, model_identity.clone());
    let main_scope_id = main_observability.scope_id.clone();

    let submit_for_pr: Option<SubmitForPrCallback> = submit_for_pr
        .filter(|_| super::submit_for_pr_available(context))
        .map(|host| bind_submit_for_pr_host(host, context, cwd));
    let mut tools =
        tool_registry_for_context(capability, context, cwd, submit_for_pr, forge_context);
    let codebase_memory_guidance = codebase_memory.append_to_registry(&mut tools);
    if enable_subagents {
        tools = add_subagents(
            handle.clone(),
            tools,
            provider_config,
            &stream_options,
            cwd,
            &scope_factory,
            &main_scope_id,
            operation_limits,
        );
    }

    // Compose both prompt turns only after the provider registry is final. All
    // optional named guidance is derived from this exact registry, which is
    // moved unchanged into SubAgent below.
    let mut role_prompt = system_prompt_with_registry(
        capability,
        &context.allowed_verdicts,
        &context.verdict_contracts,
        &tools,
    );
    if let Some(section) = codebase_memory_guidance.prompt_section_for_registry(&tools) {
        role_prompt.push_str(section);
    }
    let user = user_context_with_registry(context, &tools);
    let overlays = PromptOverlays::load(config_dir, cwd, capability);
    let turns = overlays.compose_turns(
        &role_prompt,
        &user,
        provider_config.required_system_identity(),
    );

    let sub_agent = temper_agent_core::SubAgent {
        system_prompt: Some(turns.system),
        user_message: turns.user,
        tools,
        max_iterations,
        operation_limits,
        provider,
        stream_options,
    };
    // Salient-arg preview for ToolStart human lines: computed in the pure core
    // (where the raw call args live) via this shell-supplied closure, which owns
    // the workspace `cwd` for repo-relative paths (agent-log-cleanup plan, B/D).
    let arg_preview = crate::usage::tool_arg_preview_hook(cwd.to_path_buf());
    let model_id = provider_config.model_id().to_string();
    let outcome = async {
        let (_control, run) = temper_agent_core::run_sub_agent_controllable_with_observability(
            handle.clone(),
            sub_agent,
            main_observability.observability,
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
    validate_verdict_contract(
        &result,
        &context.verdict_contracts,
        &context.source_metadata,
    )?;
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
