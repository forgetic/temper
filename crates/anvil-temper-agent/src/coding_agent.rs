//! The coding workspace agent, on anvil's native loop + tongs tools.
//!
//! This module implements temper's external coding-workspace command
//! (`TEMPER_CODING_WORKSPACE_COMMAND`). Where [`crate::decision`] runs a
//! single tool-less turn that only *decides*, this module runs a tool-using
//! agent that *acts*: it reads the work-item context temper prepared, runs a
//! real LLM agent loop with a [`tongs::tools::ToolRegistry`] scoped to the
//! checkout, and produces the ADR 0022 work product (a working-tree diff
//! and/or a verdict) for the role.
//!
//! # Protocol
//!
//! temper writes a context JSON file and names it with the
//! `TEMPER_CODING_WORKSPACE_CONTEXT` env var, runs the command in the prepared
//! checkout (cwd), and reads a result JSON file back from the path named by
//! `TEMPER_CODING_WORKSPACE_RESULT`. The result shape is temper's
//! `WorkspaceResult` (`{ verdict?, summary?, body?, review_body?, labels?,
//! children? }`); see [`WorkspaceResult`]. Reading the context and writing the
//! result is the binary's job ([`crate::coding_agent`] only models and runs the
//! agent); this module owns the schema and the agent loop.
//!
//! # Capability / role awareness
//!
//! The three reference-delivery roles map to distinct capabilities:
//!
//! - **engineer** (`coding_workspace`): edit tools; implement the issue, leaving
//!   a real product diff in the working tree. No verdict on success (the head
//!   path ⇒ `open_pr`); verdict `needs_architect` when it cannot be implemented
//!   as specified.
//! - **architect** (`triage_workspace`): read-only analysis; verdict
//!   `ready_code` / `needs_design` with an authored `body`, or `needs_breakdown`
//!   with `children`.
//! - **reviewer** (`review_workspace`): read-only diff + CI; verdict `approve`,
//!   or `changes` with an authored `review_body`, or `escalate`.
//!
//! When temper surfaces the action's declared verdict vocabulary
//! (`allowed_verdicts`, W3), the role is constrained to exactly that option set
//! instead of the broad per-role menu above, and a verdict outside it is
//! rejected with a clear message. A single-outcome triage (e.g.
//! `["ready_code"]`, as in the `basic-delivery` example) thereby has exactly one
//! choice. An empty vocabulary falls back to the per-role menu (back-compat).

use std::path::Path;

use crate::prompt_overlays::PromptOverlays;
use crate::provider::{ProviderConfig, ProviderError};
use tongs::model::ContentBlock;
use tongs::tools::{
    ToolRegistry, create_bash_tool, create_edit_tool, create_find_tool, create_grep_tool,
    create_ls_tool, create_read_tool, create_write_tool,
};

/// Default ceiling on tool-using iterations for one workspace run. The agent
/// must do real multi-step work (read, edit, verify) on substantial work items,
/// so this is well above the tool-less decision path's ceiling of 1, but bounded
/// so a confused run cannot loop forever. Raised to 250 so the engineer can take
/// larger, self-contained work items without exhausting the budget mid-run (we
/// otherwise pay the per-round-trip cost of over-splitting issues).
pub const DEFAULT_MAX_ITERATIONS: usize = 250;

// ---------------------------------------------------------------------------
// Wire DTOs — the worker ↔ agent process protocol.
//
// The context (input, `$TEMPER_CODING_WORKSPACE_CONTEXT`) and result (output,
// `$TEMPER_CODING_WORKSPACE_RESULT`) shapes are owned by the serde-only
// `smith-agent-protocol` crate — the contract a third-party agent speaks and
// the worker consumes without linking anvil's internals. We re-export them here
// so this crate's API (and all its callers) are unchanged by the move. The
// result shape must still match temper's `WorkspaceResult` /
// `WorkspaceResultChild` exactly (temper deserializes with
// `deny_unknown_fields`).
// ---------------------------------------------------------------------------

pub use smith_agent_protocol::{
    WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceResult,
    WorkspaceResultChild, WorkspaceWorkItem,
};

// ---------------------------------------------------------------------------
// Role → capability mapping.
// ---------------------------------------------------------------------------

/// The capability a role runs with. Engineer mutates the checkout; architect and
/// reviewer are read-only analysts that emit a verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    /// Edit tools; leaves a product diff. Maps to the engineer role.
    CodingWorkspace,
    /// Read-only analysis; emits a verdict + authored body / children.
    TriageWorkspace,
    /// Read-only diff + CI review; emits an approve / changes / escalate verdict.
    ReviewWorkspace,
}

impl Capability {
    /// Maps a workflow role id to its capability. Unknown roles default to the
    /// read-only triage capability so an unexpected role can never silently
    /// mutate the checkout.
    pub fn for_role(role: &str) -> Self {
        match role {
            "engineer" => Capability::CodingWorkspace,
            "reviewer" => Capability::ReviewWorkspace,
            _ => Capability::TriageWorkspace,
        }
    }

    /// Whether the capability is allowed to mutate the working tree.
    pub fn is_writable(self) -> bool {
        matches!(self, Capability::CodingWorkspace)
    }
}

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Why a coding-workspace run could not produce a result.
#[derive(Debug)]
pub enum CodingAgentError {
    /// Building the provider or loading credentials failed.
    Provider(ProviderError),
    /// The SDK agent run failed (network, provider rejection, abort).
    Run(String),
    /// The agent stopped with an error stop reason.
    AgentStopped(String),
    /// The model's reply was not the expected JSON result object.
    Parse { snippet: String, error: String },
    /// A writable (engineer) run finished without leaving a product diff and
    /// without a routing verdict — there is nothing for temper to land.
    NoProduct,
    /// The model emitted a verdict that is not in the action's declared verdict
    /// vocabulary (W3). The engine would reject it as an undeclared verdict; we
    /// fail earlier here with a clearer message naming the allowed set.
    UndeclaredVerdict {
        emitted: String,
        allowed: Vec<String>,
    },
}

impl std::fmt::Display for CodingAgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodingAgentError::Provider(error) => write!(formatter, "{error}"),
            CodingAgentError::Run(message) => write!(formatter, "LLM run failed: {message}"),
            CodingAgentError::AgentStopped(reason) => {
                write!(formatter, "agent stopped abnormally: {reason}")
            }
            CodingAgentError::Parse { snippet, error } => {
                write!(
                    formatter,
                    "could not parse agent result ({error}): {snippet}"
                )
            }
            CodingAgentError::NoProduct => formatter.write_str(
                "engineer run produced no product diff and emitted no verdict; nothing to land",
            ),
            CodingAgentError::UndeclaredVerdict { emitted, allowed } => write!(
                formatter,
                "agent emitted undeclared verdict `{emitted}`; this workflow step allows only: {}",
                allowed
                    .iter()
                    .map(|verdict| format!("`{verdict}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for CodingAgentError {}

impl From<ProviderError> for CodingAgentError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

// ---------------------------------------------------------------------------
// Prompt construction.
// ---------------------------------------------------------------------------

/// Builds the role system prompt for a capability.
///
/// `allowed_verdicts` is the workflow-declared verdict vocabulary surfaced by
/// temper (W3). When non-empty it is rendered as an authoritative constraint:
/// the role must emit exactly one of those verdicts (or, for the engineer, the
/// no-verdict head path) and nothing else. This is the principled "the workflow
/// defines the role's only options" mechanism — a single-outcome triage
/// (`["ready_code"]`) thereby collapses to one choice. When empty the agent
/// falls back to its built-in per-role verdict menu (back-compat with an older
/// temper that does not surface the vocabulary).
pub fn system_prompt(capability: Capability, allowed_verdicts: &[String]) -> String {
    let mut prompt = String::from(
        "You are Anvil, an autonomous software engineering agent running one \
         workspace turn inside a Temper workflow. You operate on a real Git \
         checkout using the provided file and shell tools. Work carefully and \
         deterministically; never invent files you have not inspected.\n\n",
    );

    match capability {
        Capability::CodingWorkspace => prompt.push_str(
            "ROLE: engineer (coding_workspace capability).\n\
             - Implement the work item as specified, leaving a real, \
             non-bookkeeping product diff in the working tree.\n\
             - Edit and create real source/docs/test files. Do NOT create \
             bookkeeping-only diffs such as `.temper-pr-prep` or `.temper-ci` \
             changes.\n\
             - Do NOT run git commit, git push, or open a PR: the harness commits, \
             pushes, and opens the PR from your working-tree diff.\n\
             - On success, emit NO verdict (the head path opens the PR). Only emit \
             verdict `needs_architect` if the item genuinely cannot be implemented \
             as specified, explaining why in `summary`.\n",
        ),
        Capability::TriageWorkspace => prompt.push_str(
            "ROLE: architect (triage_workspace capability).\n\
             - Read-only analysis: inspect the repository, but make NO edits to \
             the working tree.\n\
             - Emit exactly one verdict:\n\
             - `ready_code` with an authored `body` (a precise, implementable \
             code spec) when the item is ready to be built;\n\
             - `needs_design` with an authored `body` (a design proposal) when \
             design work is required first;\n\
             - `needs_breakdown` with a `children` list (each: slug, title, body, \
             labels, depends_on, and optional target_repo as an owner/name \
             repository path when the intake plan names target repositories) \
             when the item must be split into child issues.\n",
        ),
        Capability::ReviewWorkspace => prompt.push_str(
            "ROLE: reviewer (review_workspace capability).\n\
             - Read-only review: inspect the actual diff and CI result, not just \
             the PR summary. Make NO edits to the working tree.\n\
             - The working tree is checked out at the pull request's head. Compare \
             against the base branch from the context file (git diff \
             origin/<base_branch>...HEAD, git log origin/<base_branch>..HEAD).\n\
             - Emit exactly one verdict:\n\
             - `approve` when the change satisfies the contract and has a \
             meaningful, correct implementation diff;\n\
             - `changes` with an authored `review_body` when the change is \
             incomplete, unsafe, contradicts the contract, or is bookkeeping-only;\n\
             - `escalate` when the decision exceeds a static review (explain in \
             `summary`).\n",
        ),
    }

    // W3: when temper surfaces the action's declared verdict vocabulary,
    // constrain the role to exactly that option set. This overrides the broader
    // per-role menu above so the role can never emit a verdict the engine would
    // reject as undeclared. The engineer's head path (no verdict) is always
    // allowed in addition to any declared verdicts.
    if !allowed_verdicts.is_empty() {
        let rendered = allowed_verdicts
            .iter()
            .map(|verdict| format!("`{verdict}`"))
            .collect::<Vec<_>>()
            .join(", ");
        prompt.push_str(&format!(
            "\nVERDICT CONSTRAINT (authoritative): this workflow step declares \
             exactly these verdicts: {rendered}. You MUST emit one of them and \
             MUST NOT emit any other verdict, even if a verdict named above \
             seems wrong — pick the closest declared option."
        ));
        if matches!(capability, Capability::CodingWorkspace) {
            prompt.push_str(
                " As the engineer you may also take the no-verdict head path \
                 (leave a product diff and emit no verdict).",
            );
        } else if allowed_verdicts.len() == 1 {
            prompt.push_str(&format!(
                " This step has a SINGLE declared outcome, so your only choice is \
                 to emit verdict `{}` (with the fields that verdict requires).",
                allowed_verdicts[0]
            ));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "\nEFFICIENCY:\n\
         - Batch independent tool calls into a single response: read-only tools \
         (read, ls, grep, find, investigate) run in parallel when emitted \
         together, which is much faster than one call per turn.\n\
         - Write each new file completely in one `write` call instead of many \
         incremental edits.\n\
         - Verify with one focused command (e.g. run the test suite once after \
         the implementation is in place) rather than re-checking after every \
         small step; do not re-run checks when nothing has changed.\n\
         - Do not re-read files you just wrote, and do not use bash for things \
         a dedicated tool already does.\n",
    );

    prompt.push_str(
        "\nWhen you have finished using tools, your FINAL message must be a single \
         JSON object (and nothing else) describing the result, with these \
         optional fields: `verdict` (string), `summary` (string), `body` \
         (string), `review_body` (string), `labels` (array of strings), and \
         `children` (array of {slug, title, body, labels, depends_on, \
         target_repo?}). Omit \
         fields you are not using. For the engineer success path, emit `{\"summary\": \
         \"...\"}` with no `verdict`. Do not wrap the JSON in prose or code fences.",
    );

    prompt
}

/// Builds the user-turn context describing the concrete work item.
pub fn user_context(context: &WorkspaceContext) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "Repository: {}/{} (default branch: {})\n",
        context.repository.owner, context.repository.name, context.repository.default_branch
    ));
    text.push_str(&format!(
        "Role: {}  Queue: {}  Kind: {}\n",
        context.work_item.role, context.work_item.queue, context.work_item.kind
    ));
    text.push_str(&format!("Target: {}\n", context.work_item.target));
    text.push_str(&format!("Base branch: {}\n", context.base_branch));
    text.push_str(&format!("Branch hint: {}\n", context.branch_hint));
    text.push_str(&format!("Correlation key: {}\n", context.correlation_key));
    if let Some(checkout) = &context.checkout {
        text.push_str(&format!("Checkout mode: {checkout}\n"));
    }

    if let Some(role_guidance) = &context.guidance.role_guidance {
        text.push_str(&format!("\nRole guidance:\n{role_guidance}\n"));
    }
    if let Some(tool_guidance) = &context.guidance.tool_guidance {
        text.push_str(&format!("\nTool guidance:\n{tool_guidance}\n"));
    }
    if !context.guidance.tool_constraints.is_empty() {
        text.push_str("\nTool constraints:\n");
        for constraint in &context.guidance.tool_constraints {
            text.push_str(&format!("- {constraint}\n"));
        }
    }

    text.push_str("\nWork item context (JSON):\n");
    text.push_str(&context.work_item.context);
    text.push('\n');

    text
}

// ---------------------------------------------------------------------------
// Tool registry.
// ---------------------------------------------------------------------------

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

/// Guidance appended to the role prompt when the `investigate` sub-agent tool
/// is registered: tells the model when delegating beats searching inline and
/// that several investigations can run concurrently.
const SUBAGENT_GUIDANCE: &str = "\nSUB-AGENTS:\n\
    - The `investigate` tool delegates a read-only repository investigation to \
    a sub-agent that searches and reads files for you, returning a focused \
    report. Use it when answering a question would mean sweeping many files or \
    directories (architecture mapping, finding all usages/conventions, \
    auditing a module) — you get the conclusion without filling your own \
    context with file dumps.\n\
    - Investigations run concurrently: when you have several independent \
    questions, emit several `investigate` calls in ONE response instead of \
    asking one at a time.\n\
    - Give each sub-agent a self-contained task description: what to find, \
    where to look first, and what the report must answer. Sub-agents cannot \
    see your conversation.\n\
    - For a single-fact lookup (one known file or symbol), use read/grep \
    directly instead of a sub-agent.\n";

/// Read-only system prompt for the `investigate` sub-agent.
const INVESTIGATE_SUBAGENT_PROMPT: &str = "You are an investigation sub-agent. \
    Read the repository with the provided read-only tools and answer the task \
    concisely. Make NO edits. Your final message is your report back to the \
    calling agent.";

/// Adds an `investigate` sub-agent tool to a coding tool registry.
///
/// The tool delegates a read-only investigation to a nested sub-agent scoped to
/// the same checkout `cwd`, talking to the same provider. It declares read-only
/// effects, so the parent agent can fan out several investigations in parallel
/// and they cannot mutate the engineer's working tree.
fn add_investigate_subagent(
    mut base: ToolRegistry,
    provider_config: &ProviderConfig,
    stream_options: &tongs::provider::StreamOptions,
    cwd: &Path,
    totals: &std::sync::Arc<crate::usage::UsageTotals>,
) -> ToolRegistry {
    // Capture what the nested sub-agent needs. The factory runs per call.
    let provider_config = provider_config.clone();
    let stream_options = stream_options.clone();
    let cwd = cwd.to_path_buf();
    let factory: anvil_agent::SubAgentFactory = std::sync::Arc::new(move |task: String| {
        // Build a fresh provider for the nested run (cheap; reuses the resolved
        // bearer in stream_options).
        let provider = provider_config
            .build_provider()
            .expect("sub-agent provider builds (parent already built one)");
        anvil_agent::SubAgent {
            system_prompt: Some(INVESTIGATE_SUBAGENT_PROMPT.to_string()),
            user_message: task,
            tools: ToolRegistry::from_tools(vec![
                create_read_tool(&cwd),
                create_ls_tool(&cwd),
                create_grep_tool(&cwd),
                create_find_tool(&cwd),
            ]),
            max_iterations: 12,
            provider,
            stream_options: stream_options.clone(),
        }
    });
    base.push(Box::new(
        anvil_agent::SubAgentTool::new(
            "investigate",
            "Delegate a read-only investigation of the repository to a sub-agent. \
             Input: { task: string }. Returns the sub-agent's findings. Safe to call \
             several at once.",
            tongs::tools::ToolEffects::read(),
            factory,
        )
        .with_events(std::sync::Arc::new(crate::usage::UsageLogger::new(
            "investigate",
            std::sync::Arc::clone(totals),
        ))),
    ));
    base
}

// ---------------------------------------------------------------------------
// Agent run.
// ---------------------------------------------------------------------------

/// Runs one capability/role-aware coding-workspace turn on anvil's native
/// sans-IO agent loop ([`anvil_agent::run_sub_agent`]).
///
/// Builds the role prompt + overlays, the role's tongs tools scoped to `cwd`,
/// and per-request stream options; runs the deterministic
/// [`anvil_agent::AgentMachine`] driven by a skein shell; parses the model's
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
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
) -> Result<WorkspaceResult, CodingAgentError> {
    run_coding_agent_native_with_options(
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
pub async fn run_coding_agent_native_with_options(
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
) -> Result<WorkspaceResult, CodingAgentError> {
    run_coding_agent_native_with_hooks(
        provider_config,
        context,
        cwd,
        max_iterations,
        config_dir,
        enable_subagents,
        None,
        None,
    )
    .await
}

/// [`run_coding_agent_native_with_options`] with the phase-6b hooks: an
/// optional resume note appended to the user turn (a re-dispatched agent
/// continuing a checkpointed branch) and an optional [`anvil_agent::TurnHook`]
/// awaited before each model call (the workspace checkpointer).
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_agent_native_with_hooks(
    provider_config: &ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    max_iterations: usize,
    config_dir: Option<&Path>,
    enable_subagents: bool,
    resume_note: Option<&str>,
    turn_hook: Option<std::sync::Arc<dyn anvil_agent::TurnHook>>,
) -> Result<WorkspaceResult, CodingAgentError> {
    let capability = Capability::for_role(&context.work_item.role);
    let provider = provider_config.build_provider()?;

    let mut role_prompt = system_prompt(capability, &context.allowed_verdicts);
    if enable_subagents {
        role_prompt.push_str(SUBAGENT_GUIDANCE);
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
    let events: std::sync::Arc<dyn anvil_agent::EventSink> = std::sync::Arc::new(
        crate::usage::UsageLogger::new(crate::usage::MAIN_SCOPE, std::sync::Arc::clone(&totals)),
    );

    let mut tools = tool_registry(capability, cwd);
    if enable_subagents {
        tools = add_investigate_subagent(tools, provider_config, &stream_options, cwd, &totals);
    }

    let sub_agent = anvil_agent::SubAgent {
        system_prompt: Some(turns.system),
        user_message: turns.user,
        tools,
        max_iterations,
        provider,
        stream_options,
    };
    let outcome = async {
        let (_control, run) =
            anvil_agent::run_sub_agent_controllable_with_hook(sub_agent, events, turn_hook)?;
        run.await
    }
    .await
    .map_err(|error| CodingAgentError::Run(error.to_string()))?;
    totals.emit_summary();

    if matches!(outcome.stop, anvil_agent::AgentStop::ModelError) {
        return Err(CodingAgentError::AgentStopped(
            outcome
                .final_message
                .error_message
                .clone()
                .unwrap_or_else(|| "provider reported an error stop".to_string()),
        ));
    }

    let text = collect_text(&outcome.final_message.content);
    let result = parse_result(&text)?;
    validate_verdict_vocabulary(&result, &context.allowed_verdicts)?;
    validate_contract(capability, &result, cwd, &context.base_branch)?;
    Ok(result)
}

/// Concatenates the assistant message's text blocks (ignoring thinking/tool
/// blocks).
fn collect_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Parses the model's reply into a [`WorkspaceResult`], tolerating a code-fenced
/// or prose-wrapped JSON object. An empty / no-object reply is treated as an
/// empty head-path result (no verdict, no diff claim) so the contract check is
/// the single authority on whether that is acceptable.
fn parse_result(text: &str) -> Result<WorkspaceResult, CodingAgentError> {
    let Some(candidate) = extract_json_object(text) else {
        if text.trim().is_empty() {
            return Ok(WorkspaceResult::default());
        }
        return Err(CodingAgentError::Parse {
            snippet: snippet(text),
            error: "no JSON object found in reply".to_string(),
        });
    };
    serde_json::from_str::<WorkspaceResult>(&candidate).map_err(|error| CodingAgentError::Parse {
        snippet: snippet(text),
        error: error.to_string(),
    })
}

/// Rejects a verdict that is not in the action's declared vocabulary (W3).
///
/// When `allowed_verdicts` is non-empty, any verdict the model emits must be one
/// of them; the engine would otherwise fail the tick with an "undeclared
/// verdict" error, so we surface a clearer one here. A result with no verdict
/// (the engineer head path) and an empty `allowed_verdicts` (no declared
/// vocabulary, or an older temper) both pass through unchecked.
fn validate_verdict_vocabulary(
    result: &WorkspaceResult,
    allowed_verdicts: &[String],
) -> Result<(), CodingAgentError> {
    if allowed_verdicts.is_empty() {
        return Ok(());
    }
    let Some(verdict) = result
        .verdict
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Ok(());
    };
    if allowed_verdicts.iter().any(|allowed| allowed == verdict) {
        Ok(())
    } else {
        Err(CodingAgentError::UndeclaredVerdict {
            emitted: verdict.to_string(),
            allowed: allowed_verdicts.to_vec(),
        })
    }
}

/// Enforces the role contract that temper relies on: an engineer (writable)
/// run that emits no verdict must have left a real product diff in the working
/// tree, otherwise there is nothing to land. Read-only roles need a verdict.
fn validate_contract(
    capability: Capability,
    result: &WorkspaceResult,
    cwd: &Path,
    base_branch: &str,
) -> Result<(), CodingAgentError> {
    let has_verdict = result
        .verdict
        .as_deref()
        .map(|verdict| !verdict.trim().is_empty())
        .unwrap_or(false);

    match capability {
        Capability::CodingWorkspace => {
            if has_verdict {
                return Ok(());
            }
            // A checkpointing run (phase 6b) may have committed and pushed
            // its whole product, leaving a clean tree: commits beyond the
            // base branch count as a product too.
            if working_tree_has_changes(cwd) || commits_ahead_of_base(cwd, base_branch) {
                Ok(())
            } else {
                Err(CodingAgentError::NoProduct)
            }
        }
        Capability::TriageWorkspace | Capability::ReviewWorkspace => {
            if has_verdict {
                Ok(())
            } else {
                Err(CodingAgentError::AgentStopped(
                    "read-only role finished without emitting a verdict".to_string(),
                ))
            }
        }
    }
}

/// Returns true when `HEAD` carries commits beyond `origin/<base_branch>` in
/// `cwd` (checkpoint commits a phase-6b run pushed mid-run). Falls back to
/// `false` when git cannot answer, leaving the working-tree check decisive.
fn commits_ahead_of_base(cwd: &Path, base_branch: &str) -> bool {
    let base_branch = base_branch.trim();
    if base_branch.is_empty() {
        return false;
    }
    std::process::Command::new("git")
        .arg("rev-list")
        .arg("--count")
        .arg(format!("origin/{base_branch}..HEAD"))
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|count| count.trim() != "0")
        .unwrap_or(false)
}

/// Returns true when `git status --porcelain` reports any change in `cwd`.
/// Falls back to `false` when git cannot be invoked, which the contract check
/// then surfaces as [`CodingAgentError::NoProduct`].
fn working_tree_has_changes(cwd: &Path) -> bool {
    std::process::Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--untracked-files=all")
        .current_dir(cwd)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

/// Returns the first balanced top-level `{...}` substring, if any. Shares the
/// brace-matching logic with [`crate::decision`] but is kept local to avoid a
/// cross-module dependency on a private helper.
fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=start + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// A short, single-line snippet of the model reply for error messages.
fn snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() > 200 {
        format!("{}…", &collapsed[..200])
    } else {
        collapsed
    }
}

#[cfg(test)]
#[path = "coding_agent_tests.rs"]
mod tests;
