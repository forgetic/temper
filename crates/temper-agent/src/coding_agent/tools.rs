//! Tool registry, the model-driven `checkpoint` tool, and the read-only
//! sub-agent roles (`investigate` / `delegate`) the coding agent can fan out to.

use std::path::Path;

use crate::provider::ProviderConfig;
use tongs::tools::{
    ToolRegistry, create_bash_tool, create_edit_tool, create_find_tool, create_grep_tool,
    create_ls_tool, create_read_tool, create_write_tool,
};

use super::Capability;

/// Orchestration callback the `checkpoint` tool invokes: commit + push the
/// current work as a coherent, labeled checkpoint, returning the pushed head sha
/// (`None` when nothing changed). Implemented by the worker/agent host (which
/// owns the git credentials); the model only *decides when* to checkpoint by
/// calling the tool, keeping the push token out of the model's hands.
#[async_trait::async_trait]
pub trait CheckpointHook: Send + Sync {
    async fn checkpoint(&self, label: &str) -> Result<Option<String>, String>;
}

/// The model-facing `checkpoint` tool: at a coherent sub-milestone the agent
/// calls it to have the host commit + push its work so far. The push happens in
/// orchestration (via [`CheckpointHook`]), not in the model.
pub(super) struct CheckpointTool {
    pub(super) hook: std::sync::Arc<dyn CheckpointHook>,
}

#[async_trait::async_trait]
impl tongs::tools::Tool for CheckpointTool {
    fn name(&self) -> &str {
        "checkpoint"
    }

    fn description(&self) -> &str {
        "Commit and push your work so far as a coherent checkpoint. Call this \
         after completing a meaningful sub-milestone (e.g. a failing test added, \
         a function implemented, a bug fixed) — NOT after every edit. The host \
         performs the commit and push for you; you only choose when. Pass a \
         short imperative `label` describing what you just finished."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "label": {
                    "type": "string",
                    "description": "Short imperative summary of the sub-milestone just completed, e.g. 'add failing test for parser'"
                }
            },
            "required": ["label"]
        })
    }

    fn effects(&self) -> tongs::tools::ToolEffects {
        // It commits and pushes: process (git) + network.
        tongs::tools::ToolEffects {
            reads: false,
            writes: false,
            network: true,
            process: true,
        }
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(tongs::tools::ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<tongs::tools::ToolOutput> {
        let label = input
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("checkpoint")
            .to_string();
        match self.hook.checkpoint(&label).await {
            Ok(Some(sha)) => Ok(tongs::tools::ToolOutput::text(format!(
                "Checkpointed: committed and pushed \"{label}\" at {sha}."
            ))),
            Ok(None) => Ok(tongs::tools::ToolOutput::text(
                "Nothing to checkpoint: no changes since the last checkpoint.",
            )),
            Err(error) => Err(tongs::Error::Tool(format!("checkpoint failed: {error}"))),
        }
    }
}

/// Builds the tool registry for a capability, scoped to `cwd`.
///
/// The engineer (writable) capability gets the full edit toolset; the read-only
/// capabilities get inspection tools plus bash (so they can `git diff`,
/// `git log`, inspect CI artifacts, etc.) but no file-writing tools.
pub fn tool_registry(capability: Capability, cwd: &Path) -> ToolRegistry {
    ToolRegistry::from_tools(coding_tools_vec(capability, cwd))
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

/// Guidance appended to the role prompt when the `checkpoint` tool is wired.
pub(super) const CHECKPOINT_GUIDANCE: &str = "\nCHECKPOINTS:\n\
    - You have a `checkpoint` tool. Call it when you finish a coherent \
    sub-milestone (a failing test added, a function implemented, a bug fixed) — \
    NOT after every edit, and not for trivial intermediate states. The host \
    commits and pushes your work for you (you must not run git yourself); pass a \
    short imperative `label` for what you just finished.\n\
    - Checkpointing makes your progress durable: if the run is interrupted, work \
    you have checkpointed is recovered and you resume from it. Aim for a few \
    meaningful checkpoints over a task rather than many tiny ones or none.\n";

/// Guidance appended to the role prompt when the sub-agent tools are
/// registered: tells the model which sub-agent to delegate to, that several run
/// concurrently, and how to write a self-contained task. Mirrors Claude Code,
/// which offers a cheap read-only `Explore` sub-agent and a heavier
/// `general-purpose` one and lets the orchestrator pick by *type* — the type
/// then transitively selects the model (here: `investigate` → sub-agent tier,
/// `delegate` → main model).
pub(super) const SUBAGENT_GUIDANCE: &str = "\nSUB-AGENTS:\n\
    - You have two sub-agent tools. Both read and search the repository and \
    return a focused report; neither can edit the working tree, and several of \
    either can run concurrently (emit several calls in ONE response when the \
    questions are independent).\n\
    - `investigate`: a fast, cheap read-only searcher (read/ls/grep/find). Use \
    it for the common case — sweeping many files or directories to answer a \
    question (architecture mapping, finding all usages/conventions, locating \
    code). You get the conclusion without filling your own context with file \
    dumps.\n\
    - `delegate`: a heavier reviewer that also has `bash` (for read-only \
    inspection like `git diff`/`git log`/running a check) and runs on the full \
    model. Use it for self-contained analysis that needs judgement, not just \
    search — auditing a module for bugs, reviewing a diff, assessing a design.\n\
    - Give each sub-agent a self-contained task: what to find, where to look \
    first, and what the report must answer. Sub-agents cannot see your \
    conversation.\n\
    - For a single-fact lookup (one known file or symbol), use read/grep \
    directly instead of a sub-agent.\n";

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
    totals: &std::sync::Arc<crate::usage::UsageTotals>,
) -> ToolRegistry {
    for spec in subagent_specs() {
        base = add_one_subagent(
            handle.clone(),
            base,
            spec,
            provider_config,
            stream_options,
            cwd,
            totals,
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
    totals: &std::sync::Arc<crate::usage::UsageTotals>,
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
        .with_events(std::sync::Arc::new(crate::usage::UsageLogger::new(
            spec.name,
            std::sync::Arc::clone(totals),
        ))),
    ));
    base
}
