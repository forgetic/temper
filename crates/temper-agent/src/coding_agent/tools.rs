//! Tool registry and the read-only sub-agent roles (`investigate` / `delegate`)
//! the coding agent can fan out to.

use std::path::Path;

use crate::provider::ProviderConfig;
use tongs::tools::{
    ToolRegistry, create_bash_tool, create_edit_tool, create_find_tool, create_grep_tool,
    create_ls_tool, create_read_tool, create_write_tool,
};

use super::Capability;
use super::forge::{ForgeContextHost, ForgeGetItemTool, ForgeListRelatedTool};
use super::submit::{SubmitForPrCallback, SubmitForPrTool, submit_for_pr_available};
use temper_protocol_agent::WorkspaceContext;

/// Builds the tool registry for a capability, scoped to `cwd`.
///
/// The engineer (writable) capability gets the full edit toolset; the read-only
/// capabilities get inspection tools plus bash (so they can `git diff`,
/// `git log`, inspect CI artifacts, etc.) but no file-writing tools.
pub fn tool_registry(capability: Capability, cwd: &Path) -> ToolRegistry {
    ToolRegistry::from_tools(coding_tools_vec(capability, cwd))
}

/// Builds the tool registry for a concrete workspace context, optionally adding
/// the host-controlled `submit_for_pr` relay for writable engineer sessions.
pub(crate) fn tool_registry_for_context(
    capability: Capability,
    context: &WorkspaceContext,
    cwd: &Path,
    submit_for_pr: Option<SubmitForPrCallback>,
    forge_context: Option<ForgeContextHost>,
) -> ToolRegistry {
    let mut tools = coding_tools_vec(capability, cwd);
    if let Some(callback) = submit_for_pr.filter(|_| submit_for_pr_available(context)) {
        tools.push(Box::new(SubmitForPrTool::new(context, callback)));
    }
    if let Some(host) = forge_context {
        tools.push(Box::new(ForgeGetItemTool::new(host.clone())));
        tools.push(Box::new(ForgeListRelatedTool::new(host)));
    }
    ToolRegistry::from_tools(tools)
}

/// The base tool list for a capability (read-only inspection tools for everyone,
/// plus edit/write for the writable engineer). Returned as a `Vec` so callers
/// can append extra tools (e.g. a sub-agent tool) before building the registry.
fn coding_tools_vec(capability: Capability, cwd: &Path) -> Vec<Box<dyn tongs::tools::Tool>> {
    let mut tools = vec![
        create_read_tool(cwd),
        create_ls_tool(cwd),
        create_grep_tool(cwd),
        create_find_tool(cwd),
        create_bash_tool(cwd),
    ];
    if capability.is_writable() {
        tools.push(create_edit_tool(cwd));
        tools.push(create_write_tool(cwd));
    }
    tools
}

/// Whether the finalized provider tool registry contains `name`.
///
/// Prompt assembly uses this instead of configuration flags so optional named
/// guidance and the provider's tool manifest always describe the same surface.
pub(super) fn registry_has_tool(registry: &ToolRegistry, name: &str) -> bool {
    registry.tools().iter().any(|tool| tool.name() == name)
}

/// Concise guidance for exactly the sub-agent tools in the finalized registry.
/// Tool descriptions and schemas remain in the provider tool manifest.
pub(super) fn subagent_guidance(registry: &ToolRegistry) -> Option<String> {
    let investigate = registry_has_tool(registry, "investigate");
    let delegate = registry_has_tool(registry, "delegate");
    if !investigate && !delegate {
        return None;
    }

    let mut guidance = String::from(
        "\nSUB-AGENTS:\n\
         - Available sub-agents are read-only, can run concurrently for independent questions, \
         and need self-contained tasks because they cannot see your conversation.\n",
    );
    if investigate {
        guidance.push_str(
            "- Use `investigate` for fast repository mapping, broad searches, and usage discovery.\n",
        );
    }
    if delegate {
        guidance.push_str(
            "- Use `delegate` for self-contained analysis or review that needs judgement or read-only shell inspection.\n",
        );
    }
    Some(guidance)
}

/// System prompt for the read-only `investigate` sub-agent (the cheap-tier
/// searcher; Claude's `Explore` analog).
const INVESTIGATE_SUBAGENT_PROMPT: &str = "You are an investigation sub-agent. \
    Read the repository with the provided read-only tools and answer the task \
    concisely. Make NO edits. Your final message is your report back to the \
    calling agent.";

/// System prompt for the `delegate` sub-agent (the heavier main-model reviewer;
/// Claude's `general-purpose` analog). It has bash for read-only inspection.
const DELEGATE_SUBAGENT_PROMPT: &str = "You are a delegated analysis sub-agent. \
    Investigate the repository with the provided tools to carry out the task — \
    you may run read-only shell commands (e.g. `git diff`, `git log`, `grep`, a \
    test or lint command) via `bash`, but make NO edits to the working tree and \
    run nothing destructive. Be evidence-based and cite file:line. Your final \
    message is your report back to the calling agent.";

/// Which model tier a sub-agent role runs on.
pub(crate) enum SubAgentTier {
    /// The provider's cheaper sub-agent model (e.g. Haiku) — for the read-only
    /// searcher whose product is a focused report, not the final deliverable.
    Cheap,
    /// The same model as the orchestrator — for heavier analysis/review.
    Main,
}

/// Static description of a sub-agent role: the orchestrator picks a role by
/// calling its tool, and the role fixes the model, tools, prompt, and budget.
pub(crate) struct SubAgentSpec {
    /// Tool name the orchestrator calls.
    pub(crate) name: &'static str,
    /// Tool description shown to the orchestrator model.
    description: &'static str,
    /// The sub-agent's own system prompt.
    prompt: &'static str,
    /// Which model tier it runs on.
    pub(crate) tier: SubAgentTier,
    /// Whether it gets a `bash` tool (read-only inspection) in addition to the
    /// read/ls/grep/find set.
    pub(crate) with_bash: bool,
    /// Iteration budget. A cheap-tier model takes more, smaller steps.
    max_iterations: usize,
}

/// The sub-agent roles offered to the coding agent, mirroring Claude Code's
/// `Explore` (cheap, read-only) + `general-purpose` (main model, heavier) split.
/// The orchestrator chooses a role by calling its tool; the role determines the
/// model — the LLM does not pick a model directly, exactly as in Claude Code.
pub(crate) fn subagent_specs() -> &'static [SubAgentSpec] {
    &[
        SubAgentSpec {
            name: "investigate",
            description: "Delegate a read-only investigation of the repository to a fast, cheap \
                 sub-agent that searches and reads files. Input: { task: string }. Returns \
                 the sub-agent's findings. Safe to call several at once.",
            prompt: INVESTIGATE_SUBAGENT_PROMPT,
            tier: SubAgentTier::Cheap,
            // Smaller model, more steps per investigation.
            max_iterations: 24,
            with_bash: false,
        },
        SubAgentSpec {
            name: "delegate",
            description: "Delegate a self-contained analysis or review to a sub-agent that reads \
                 and searches the repo and may run read-only shell commands (git diff/log, \
                 grep, a test/lint). Runs on the full model — use for work needing judgement \
                 (audit a module, review a diff, assess a design), not plain search. Input: \
                 { task: string }. Returns the sub-agent's report. Safe to call several at once.",
            prompt: DELEGATE_SUBAGENT_PROMPT,
            tier: SubAgentTier::Main,
            max_iterations: 16,
            with_bash: true,
        },
    ]
}

/// Adds the sub-agent tools (see [`subagent_specs`]) to a coding tool registry.
///
/// Each tool delegates to a nested sub-agent scoped to the same checkout `cwd`
/// and talking to the same provider. They declare read-only effects, so the
/// parent agent can fan several out in parallel and they cannot mutate the
/// working tree (the read-only ones cannot at all; the bash-capable `delegate`
/// is prompt-constrained to read-only inspection, matching Claude's parallel
/// `general-purpose` reviewers).
pub(crate) fn add_subagents(
    handle: skein::runtime::RuntimeHandle,
    mut base: ToolRegistry,
    provider_config: &ProviderConfig,
    stream_options: &tongs::provider::StreamOptions,
    cwd: &Path,
    scope_factory: &crate::activity::ScopeFactory,
    parent_scope_id: &str,
) -> ToolRegistry {
    for spec in subagent_specs() {
        base = add_one_subagent(
            handle.clone(),
            base,
            spec,
            provider_config,
            stream_options,
            cwd,
            (scope_factory, parent_scope_id),
        );
    }
    base
}

/// Wires a single sub-agent role into the registry.
fn add_one_subagent(
    handle: skein::runtime::RuntimeHandle,
    mut base: ToolRegistry,
    spec: &'static SubAgentSpec,
    provider_config: &ProviderConfig,
    stream_options: &tongs::provider::StreamOptions,
    cwd: &Path,
    scope: (&crate::activity::ScopeFactory, &str),
) -> ToolRegistry {
    // The role's model tier. The cheap tier (e.g. Haiku) is for the read-only
    // searcher whose product is a focused report and which dominates token spend
    // on a large fan-out; the main tier is for heavier analysis. This mirrors
    // Claude routing `Explore` to Haiku and `general-purpose` to the main model.
    let provider_config = match spec.tier {
        SubAgentTier::Cheap => provider_config.with_model_id(provider_config.subagent_model_id()),
        SubAgentTier::Main => provider_config.clone(),
    };
    // Reuse the parent's resolved bearer and per-request options, but rebuild the
    // model-dependent headers for this role's model: the parent's
    // `stream_options` carried headers computed for the *main* model (e.g. the
    // 1M-context beta), which a smaller sub-agent model is not entitled to and
    // would 400 on. The bearer is shared across models on the same provider, so
    // only the headers need to change.
    let mut stream_options = stream_options.clone();
    stream_options.headers = provider_config.request_headers();
    let observer_provider = provider_config.provider_id().to_string();
    let observer_model = provider_config.model_id().to_string();
    let observer_factory = scope.0.clone();
    let observer_parent = scope.1.to_string();
    let observer_display_name = spec.name.to_string();
    let cwd = cwd.to_path_buf();
    let prompt = spec.prompt;
    let with_bash = spec.with_bash;
    let max_iterations = spec.max_iterations;
    let factory: temper_agent_core::SubAgentFactory = std::sync::Arc::new(move |task: String| {
        // Build a fresh provider for the nested run (cheap; reuses the resolved
        // bearer in stream_options).
        let provider = provider_config
            .build_provider()
            .expect("sub-agent provider builds (parent already built one)");
        let mut tools = vec![
            create_read_tool(&cwd),
            create_ls_tool(&cwd),
            create_grep_tool(&cwd),
            create_find_tool(&cwd),
        ];
        if with_bash {
            tools.push(create_bash_tool(&cwd));
        }
        temper_agent_core::SubAgent {
            system_prompt: Some(prompt.to_string()),
            user_message: task,
            tools: ToolRegistry::from_tools(tools),
            max_iterations,
            provider,
            stream_options: stream_options.clone(),
        }
    });
    base.push(Box::new(
        // Both roles declare read-only effects so the parent can fan them out in
        // parallel. The bash-capable `delegate` is prompt-constrained to
        // read-only inspection (no edits/destructive commands), matching Claude's
        // parallel `general-purpose` reviewers; this is a deliberate trust
        // decision, not an effect-system guarantee.
        temper_agent_core::SubAgentTool::new(
            handle.clone(),
            spec.name,
            spec.description,
            tongs::tools::ToolEffects::read(),
            factory,
        )
        .with_observer_factory(std::sync::Arc::new(move || {
            observer_factory
                .child(
                    observer_parent.clone(),
                    observer_display_name.clone(),
                    temper_agent_core::ModelIdentity::new(
                        observer_provider.clone(),
                        observer_model.clone(),
                    ),
                )
                .observability
        })),
    ));
    base
}
