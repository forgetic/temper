//! Role system-prompt and user-turn context construction.

mod outcomes;

use std::collections::{BTreeMap, BTreeSet};

use self::outcomes::render_workflow_outcomes;

use super::Capability;
use super::tools::{registry_has_tool, subagent_guidance};
use temper_protocol_agent::{
    ArtifactContextBundle, ArtifactReference, ArtifactRelationType, ArtifactSnapshot,
    ArtifactSummary, ArtifactType, WorkflowArtifactReference, WorkflowChildIdentity,
    WorkspaceContext,
};
use temper_verdict::VerdictContracts;
use tongs::tools::ToolRegistry;

/// Builds the role system prompt for a capability.
///
/// `allowed_verdicts` is the workflow-declared verdict vocabulary surfaced by
/// temper (W3). When non-empty, the prompt renders only those declared outcomes
/// and the requirements supplied by their [`temper_verdict::VerdictContract`] values. When
/// empty, the agent falls back to its built-in per-role verdict menu for
/// compatibility with older contexts. The engineer's no-verdict success path is
/// invariant and remains available in either case.
pub fn system_prompt(capability: Capability, allowed_verdicts: &[String]) -> String {
    system_prompt_with_contracts(capability, allowed_verdicts, &VerdictContracts::new())
}

/// Builds the role prompt with exact workflow-derived product requirements.
pub fn system_prompt_with_contracts(
    capability: Capability,
    allowed_verdicts: &[String],
    verdict_contracts: &VerdictContracts,
) -> String {
    let mut prompt = String::from(
        "You are Anvil, an autonomous software engineering agent running one \
         workspace turn inside a Temper workflow. You operate on a real Git \
         checkout using the provided file and shell tools. Work carefully and \
         deterministically; never invent files you have not inspected.\n\n\
         IMPORTANT: Your FINAL message after all tool use MUST be a single JSON \
         object (the WorkspaceResult envelope). Do NOT end with prose narration \
         — end with the JSON result.\n\n",
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
             - On success, emit NO verdict (the head path opens the PR). This \
             no-verdict success path remains available regardless of the declared \
             workflow outcome vocabulary.\n\
             - For any declared decline outcome, explain the reason in `summary`.\n\
             - Report validation in `summary` when relevant. Keep `summary` short; \
             the durable PR handoff is the success-path `title` and `body`.\n\
             - For PR repair runs (`pull_request_writable` checkout), preserve and \
             update the current implementation PR handoff from the context: on \
             success emit an updated PR `title`, a compact implementation-report \
             `body`, and a short `summary`.\n",
        ),
        Capability::TriageWorkspace => prompt.push_str(
            "ROLE: architect (triage_workspace capability).\n\
             - Read-only analysis: inspect the repository, but make NO edits to \
             the working tree.\n\
             - Analyze the work item and repository evidence carefully enough to \
             produce the workflow outcome requested below.\n",
        ),
        Capability::ReviewWorkspace => prompt.push_str(
            "ROLE: reviewer (review_workspace capability).\n\
             - Read-only review: inspect the actual diff and CI result, not just \
             the PR summary. Make NO edits to the working tree.\n\
             - The working tree is checked out at the pull request's head. Compare \
             against the base branch from the context file (git diff \
             origin/<base_branch>...HEAD, git log origin/<base_branch>..HEAD).\n",
        ),
    }

    if allowed_verdicts.is_empty() {
        render_legacy_outcomes(&mut prompt, capability);
    } else {
        render_workflow_outcomes(&mut prompt, capability, allowed_verdicts, verdict_contracts);
    }

    render_efficiency(&mut prompt, capability);

    prompt.push_str(
        "\n---\n\
         FINAL MESSAGE FORMAT (mandatory):\n\
         When you have finished using tools, your very last message must be a \
         single JSON object and nothing else — no prose before or after it, no \
         code fences, no explanation. The JSON object describes the result, with \
         these optional fields: `verdict` (string), `title` (string), `summary` \
         (string), `body` (string), `review_body` (string), `labels` (array of \
         strings), and `children` (array of {slug, title, body, kind?, labels, depends_on, \
         target_repo?}). Omit fields you are not using. For a no-verdict engineer \
         success, emit a compact current implementation report as `body` and a PR \
         title as `title`, for example \
         `{\"title\":\"Implement durable PR handoff\",\"body\":\"# Implementation report\\\\n...\",\"summary\":\"Implemented PR handoff; tests pass\"}`. \
         The body should be current and compact, not append-only; do not add a \
         hidden implementation-report block.\n\
         Do NOT wrap the JSON in prose or code fences. Do NOT narrate what you \
         are about to do — just emit the JSON result as your final message.",
    );

    prompt
}

fn render_efficiency(prompt: &mut String, capability: Capability) {
    match capability {
        Capability::CodingWorkspace => prompt.push_str(
            "\nEFFICIENCY:\n\
             - Scale discovery to the task. For non-local work, batch independent \
             status, architecture, and search calls into one response; skip ritual \
             discovery when the task is already localized.\n\
             - Read the complete likely source, test, configuration, and \
             documentation set together before editing. Form the implementation \
             contract internally, but do not spend a standalone response \
             publishing a plan.\n\
             - Group related, safe `edit` and `write` calls by cohesive \
             responsibility, normally completing the work in one to four mutation \
             responses instead of one response per file. Prefer writing a complete \
             new file in one operation over incremental fragments.\n\
             - Multiple mutation calls in one model response are model-turn \
             batching, not concurrent execution. Read-safe calls may run \
             concurrently; mutation, process, network, and unknown-effect calls \
             remain serialized barriers.\n\
             - Complete the planned source, tests, configuration, and documentation \
             deliverables before running the formatter and focused authoritative \
             test suite. Do not repeatedly check partial work.\n\
             - If validation fails, perform bounded repair and focused revalidation \
             without broad rediscovery. Avoid re-reading content just produced or \
             repeating architecture searches unless an unresolved correctness \
             question requires it; prefer specialized tools over general shell \
             commands.\n\
             - Inspect repository status, the tracked diff, and all untracked \
             deliverables together. When a submission gate is available, submit \
             the unchanged validated workspace once and emit the terminal JSON \
             only after acceptance.\n\
             - Treat 8–12 total responses, at most four mutation responses, and \
             zero validation invalidations as goals, not correctness limits. Task \
             correctness and required validation take priority.\n",
        ),
        Capability::TriageWorkspace | Capability::ReviewWorkspace => prompt.push_str(
            "\nEFFICIENCY:\n\
             - Batch independent read-only calls into a single response when the \
             available tools support parallel execution.\n\
             - When mutation tools are available, prefer creating a complete new file \
             in one operation over many incremental changes.\n\
             - Verify with one focused command (for example, run the relevant test \
             suite once after implementation) rather than re-checking after every \
             small step; do not re-run checks when nothing has changed.\n\
             - Avoid re-reading content just produced, and prefer a specialized \
             available tool over a general shell command for the same operation.\n",
        ),
    }
}

/// Builds the role prompt plus guidance for exactly the tools in the finalized
/// registry that will be sent to the provider.
pub(crate) fn system_prompt_with_registry(
    capability: Capability,
    allowed_verdicts: &[String],
    verdict_contracts: &VerdictContracts,
    registry: &ToolRegistry,
) -> String {
    let mut prompt = system_prompt_with_contracts(capability, allowed_verdicts, verdict_contracts);

    if registry_has_tool(registry, "submit_for_pr") {
        prompt.push_str(
            "\n\nSUBMIT GATE:\n\
             Before emitting the final WorkspaceResult JSON on the success path, call \
             `submit_for_pr`. If the host rejects the submission, keep the current \
             session context, fix the reported problems, and call `submit_for_pr` \
             again. Emit the terminal JSON only after the host accepts.\n",
        );
    }
    if let Some(guidance) = subagent_guidance(registry) {
        prompt.push_str(&guidance);
    }

    prompt
}

fn render_legacy_outcomes(prompt: &mut String, capability: Capability) {
    prompt.push_str("\nLEGACY FALLBACK OUTCOMES (no workflow outcomes were declared):\n");
    match capability {
        Capability::CodingWorkspace => prompt.push_str(
            "The no-verdict success path above remains preferred after a successful implementation. Otherwise emit exactly one of the following and explain the reason in `summary`:\n\
             - `needs_architect` when the item is underspecified or unimplementable as written;\n\
             - `needs_human` only when implementation requires non-agent judgment.\n",
        ),
        Capability::TriageWorkspace => prompt.push_str(
            "Emit exactly one verdict:\n\
             - `ready_code` with an authored `body` (a precise, implementable code spec) when the item is ready to be built;\n\
             - `needs_design` with an authored `body` (a design proposal) when design work is required first;\n\
             - `needs_breakdown` with a `children` list (each: slug, title, body, optional kind, labels, depends_on, and optional target_repo as an owner/name repository path when the intake plan names target repositories) when the item must be split into child issues. Omit child kind only for ordinary `code` children.\n",
        ),
        Capability::ReviewWorkspace => prompt.push_str(
            "Emit exactly one verdict:\n\
             - `approve` when the change satisfies the contract and has a meaningful, correct implementation diff;\n\
             - `changes` with an authored `review_body` when the change is incomplete, unsafe, contradicts the contract, or is bookkeeping-only;\n\
             - `escalate` when the decision exceeds a static review (explain in `summary`).\n",
        ),
    }
}

/// Builds the user-turn context describing the concrete work item.
///
/// This compatibility wrapper has no provider registry, so it omits all
/// optional named tool guidance. Production assembly uses
/// [`user_context_with_registry`].
pub fn user_context(context: &WorkspaceContext) -> String {
    user_context_inner(context, None)
}

/// Registry-aware user context used by the production run path.
pub(crate) fn user_context_with_registry(
    context: &WorkspaceContext,
    registry: &ToolRegistry,
) -> String {
    user_context_inner(context, Some(registry))
}

fn user_context_inner(context: &WorkspaceContext, registry: Option<&ToolRegistry>) -> String {
    let mut text = String::new();
    if context.repos.len() > 1 {
        text.push_str(
            "This is a COORDINATED multi-repo workspace. Your working directory is the workspace root, and each repository below is checked out into its own sibling subdirectory (the `dir:` path). ",
        );
        match context.checkout.as_deref() {
            Some("read_only" | "pull_request_read_only") => text.push_str(
                "The effective checkout is read-only, so every repository is present only for inspection and build resolution; do not modify any repository, including repositories whose manifest policy is writable. ",
            ),
            Some("writable" | "pull_request_writable") => text.push_str(
                "Edit files only inside repositories whose manifest access policy is writable; repositories whose manifest policy is read-only are present only for inspection and build resolution. ",
            ),
            None => text.push_str(
                "Under this legacy context, edit files inside the manifest-writable repositories' directories; manifest-read-only repositories are present only for inspection and build resolution. ",
            ),
            Some(_) => text.push_str(
                "Use the effective checkout authority rendered below, and never modify a repository whose manifest policy is read-only. ",
            ),
        }
        text.push_str(
            "The repositories are laid out as siblings so their inter-repo path dependencies resolve.\n\nRepositories:\n",
        );
    } else {
        text.push_str("Repository:\n");
    }
    for repo in &context.repos {
        let branch = repo
            .branch_hint
            .as_deref()
            .unwrap_or("(read-only — never pushed)");
        text.push_str(&format!(
            "- {}/{} (dir: {}/, manifest/repository access policy: {}, default branch: {}, base branch: {}, work branch: {})\n",
            repo.owner, repo.name, repo.dir, repo.access, repo.default_branch, repo.base_branch, branch
        ));
    }
    text.push_str(&format!(
        "Role: {}  Queue: {}  Action: {}  Kind: {}\n",
        context.work_item.role, context.work_item.queue, context.action, context.work_item.kind
    ));
    text.push_str(&format!("Target: {}\n", context.work_item.target));
    text.push_str(&format!("Correlation key: {}\n", context.correlation_key));
    render_checkout_authority(&mut text, context.checkout.as_deref());

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
    if let Some(action_guidance) = &context.guidance.action_guidance {
        text.push_str(&format!("\nAction guidance:\n{action_guidance}\n"));
    }

    match &context.artifact_context {
        Some(bundle) => render_artifact_context(&mut text, bundle, registry),
        None => {
            // Backward compatibility for contexts emitted before artifact bundles:
            // preserve the historical heading and singular JSON verbatim.
            text.push_str("\nWork item context (JSON):\n");
            text.push_str(&context.work_item.context);
            text.push('\n');
        }
    }

    text
}

fn render_checkout_authority(text: &mut String, checkout: Option<&str>) {
    match checkout {
        Some("writable") => text.push_str(
            "Checkout mode: writable\nEffective checkout authority: writable. Edits are permitted only in repositories whose manifest/repository access policy is `writable`; no other repository may be modified.\n",
        ),
        Some("pull_request_writable") => text.push_str(
            "Checkout mode: pull_request_writable\nEffective checkout authority: pull-request writable. Edits are permitted only in repositories whose manifest/repository access policy is `writable`; no other repository may be modified.\n",
        ),
        Some(mode @ ("read_only" | "pull_request_read_only")) => text.push_str(&format!(
            "Checkout mode: {mode}\nEffective checkout authority: read-only (`{mode}`). No repository may be modified. This overrides writable manifest/repository access policy and all branch or work-branch hints.\n"
        )),
        Some(mode) => text.push_str(&format!(
            "Checkout mode: {mode}\nEffective checkout authority: checkout mode `{mode}`, subject to manifest/repository access policy. Never modify a repository whose policy is not `writable`.\n"
        )),
        None => text.push_str(
            "Effective checkout authority: not supplied by this legacy context. Repository manifest/repository access policy and branch hints remain the available legacy authority signals.\n",
        ),
    }
}

fn render_artifact_context(
    text: &mut String,
    bundle: &ArtifactContextBundle,
    registry: Option<&ToolRegistry>,
) {
    text.push_str(&format!(
        "\nArtifact context bundle (version {}):\nRepository: {} ({})\n",
        bundle.version, bundle.repository.path, bundle.repository.id
    ));

    text.push_str("\nPrimary artifact:\n");
    render_snapshot(text, &bundle.primary);

    text.push_str("\nMandatory lineage:\n");
    if bundle.lineage.is_empty() {
        text.push_str("- No mandatory ancestors.\n");
    } else {
        for snapshot in &bundle.lineage {
            render_snapshot(text, snapshot);
        }
    }

    text.push_str("\nValidation summaries:\n");
    if bundle.validation_scope.is_empty() {
        text.push_str("- No validation dependencies or implementations were collected.\n");
    } else {
        for summary in &bundle.validation_scope {
            render_summary(text, summary);
        }
    }

    text.push_str("\nOptional body-omitted references:\n");
    if bundle.optional_references.is_empty() {
        text.push_str("- None.\n");
    } else {
        for summary in &bundle.optional_references {
            render_summary(text, summary);
        }
    }

    text.push_str("\nDiagnostics and truncation:\n");
    if bundle.diagnostics.is_empty() {
        text.push_str("- No collection diagnostics.\n");
    } else {
        for diagnostic in &bundle.diagnostics {
            let code = serde_json::to_value(diagnostic.code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string());
            let source = diagnostic
                .source
                .as_ref()
                .map(|source| format!(" ({})", reference_name(source)))
                .unwrap_or_default();
            text.push_str(&format!("- {code}{source}: {}\n", diagnostic.message));
        }
    }
    text.push_str(&format!(
        "- truncation: depth_exceeded={}, count_exceeded={}, content_truncated={}\n",
        bundle.truncation.depth_exceeded,
        bundle.truncation.count_exceeded,
        bundle.truncation.content_truncated
    ));

    let forge_get_item = registry.is_some_and(|tools| registry_has_tool(tools, "forge_get_item"));
    let forge_list_related =
        registry.is_some_and(|tools| registry_has_tool(tools, "forge_list_related"));
    if forge_get_item || forge_list_related {
        text.push_str("\nForge context follow-up:\n");
        if forge_get_item {
            text.push_str(
                "- Use `forge_get_item` for bounded read-only follow-up when an artifact body or comments are missing.\n",
            );
        }
        if forge_list_related {
            text.push_str(
                "- Use `forge_list_related` when an indirect typed relation must be followed.\n",
            );
        }
        text.push_str(
            "Pass only a configured owner/name repository and artifact identity; the host binds assignment credentials.\n",
        );
    }
}

fn render_snapshot(text: &mut String, snapshot: &ArtifactSnapshot) {
    text.push_str(&format!(
        "- {} — {} [{}] labels={}\n  Body:\n",
        reference_name(&snapshot.artifact),
        snapshot.title,
        snapshot.state,
        if snapshot.labels.is_empty() {
            "(none)".to_string()
        } else {
            snapshot.labels.join(", ")
        },
    ));
    // Keep authored Markdown byte-for-byte intact. The conditional newline is
    // only a delimiter for the renderer-owned section that follows it.
    text.push_str(&snapshot.body);
    if !snapshot.body.ends_with('\n') {
        text.push('\n');
    }

    let mut workflow_text = String::new();
    let projected = snapshot.workflow.as_ref();
    if let Some(kind) = projected
        .and_then(|workflow| populated(workflow.kind.as_deref()))
        .or_else(|| populated(snapshot.workflow_kind.as_deref()))
    {
        workflow_text.push_str(&format!("    kind: {kind}\n"));
    }
    if let Some(workflow) = projected {
        render_workflow_references(&mut workflow_text, "parents", &workflow.parents);
        render_workflow_references(&mut workflow_text, "dependencies", &workflow.dependencies);
        if let Some(target_branch) = populated(workflow.target_branch.as_deref()) {
            workflow_text.push_str(&format!("    target branch: {target_branch}\n"));
        }
        if let Some(correlation_key) = populated(workflow.correlation_key.as_deref()) {
            workflow_text.push_str(&format!("    correlation key: {correlation_key}\n"));
        }
        render_workflow_children(&mut workflow_text, &workflow.children);
    }
    if !workflow_text.is_empty() {
        text.push_str("  Workflow context:\n");
        text.push_str(&workflow_text);
    }
}

fn render_workflow_references(
    text: &mut String,
    label: &str,
    references: &[WorkflowArtifactReference],
) {
    let references = references
        .iter()
        .filter_map(|reference| {
            populated(Some(&reference.repository_id))
                .map(|repository_id| (repository_id.to_string(), reference.number))
        })
        .collect::<BTreeSet<_>>();
    if references.is_empty() {
        return;
    }
    let references = references
        .into_iter()
        .map(|(repository_id, number)| format!("{repository_id}#{number}"))
        .collect::<Vec<_>>()
        .join(", ");
    text.push_str(&format!("    {label}: {references}\n"));
}

fn render_workflow_children(text: &mut String, children: &[WorkflowChildIdentity]) {
    let mut canonical = BTreeMap::<(String, u64), (String, Option<String>)>::new();
    for child in children {
        let Some(repository_id) = populated(Some(&child.repository_id)) else {
            continue;
        };
        let title = child.title.trim().to_string();
        let state = populated(child.state.as_deref()).map(str::to_string);
        canonical
            .entry((repository_id.to_string(), child.number))
            .and_modify(|(canonical_title, canonical_state)| {
                if canonical_title.is_empty() || (!title.is_empty() && title < *canonical_title) {
                    *canonical_title = title.clone();
                }
                if let Some(state) = &state {
                    if canonical_state
                        .as_ref()
                        .is_none_or(|canonical_state| state < canonical_state)
                    {
                        *canonical_state = Some(state.clone());
                    }
                }
            })
            .or_insert((title, state));
    }
    if canonical.is_empty() {
        return;
    }

    text.push_str("    children:\n");
    for ((repository_id, number), (title, state)) in canonical {
        text.push_str(&format!("      - {repository_id}#{number}"));
        if !title.is_empty() {
            text.push_str(&format!(" — {title}"));
        }
        if let Some(state) = state {
            text.push_str(&format!(" [{state}]"));
        }
        text.push('\n');
    }
}

fn populated(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn render_summary(text: &mut String, summary: &ArtifactSummary) {
    text.push_str(&format!(
        "- {} — {} [{}] kind={} labels={} relation={} source={}\n",
        reference_name(&summary.artifact),
        summary.title,
        summary.state,
        summary.workflow_kind.as_deref().unwrap_or("(unknown)"),
        if summary.labels.is_empty() {
            "(none)".to_string()
        } else {
            summary.labels.join(", ")
        },
        relation_name(summary.relation_type),
        reference_name(&summary.source),
    ));
}

fn reference_name(reference: &ArtifactReference) -> String {
    format!(
        "{} {}#{}",
        artifact_type_name(reference.artifact_type),
        reference.repository.path,
        reference.number
    )
}

fn artifact_type_name(artifact_type: ArtifactType) -> &'static str {
    match artifact_type {
        ArtifactType::Issue => "issue",
        ArtifactType::PullRequest => "pull request",
    }
}

fn relation_name(relation: ArtifactRelationType) -> &'static str {
    match relation {
        ArtifactRelationType::Parent => "parent",
        ArtifactRelationType::Dependency => "dependency",
        ArtifactRelationType::Related => "related",
    }
}
