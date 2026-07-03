//! Role system-prompt and user-turn context construction.

use super::Capability;
use temper_protocol_agent::WorkspaceContext;

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
             - Before emitting the final WorkspaceResult JSON on the success path, \
             call the `submit_for_pr` tool. If the host responds with failure \
             gate data, keep your in-session context, fix the workspace, and \
             call `submit_for_pr` again. Only after a host success response may \
             you emit the terminal JSON.\n\
             - On success, emit NO verdict (the head path opens the PR). Only emit \
             a declared decline verdict: `needs_architect` when the item is \
             underspecified or unimplementable as written, or `needs_human` only \
             when implementation requires non-agent judgment. Explain the reason \
             in `summary`.\n\
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
             - Emit exactly one verdict:\n\
             - `ready_code` with an authored `body` (a precise, implementable \
             code spec) when the item is ready to be built;\n\
             - `needs_design` with an authored `body` (a design proposal) when \
             design work is required first;\n\
             - `needs_breakdown` with a `children` list (each: slug, title, body, \
             optional kind, labels, depends_on, and optional target_repo as an \
             owner/name repository path when the intake plan names target \
             repositories) when the item must be split into child issues. Omit \
             child kind only for ordinary `code` children.\n",
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

/// Builds the user-turn context describing the concrete work item.
pub fn user_context(context: &WorkspaceContext) -> String {
    let mut text = String::new();
    if context.repos.len() > 1 {
        text.push_str(
            "This is a COORDINATED multi-repo workspace. Your working directory is \
             the workspace root, and each repository below is checked out into its \
             own sibling subdirectory (the `dir:` path). Edit files inside the \
             writable repos' directories; the read-only repos are present only so \
             the combined build resolves — do not modify them. The repos are laid \
             out as siblings so their inter-repo path dependencies resolve.\n\n\
             Repositories:\n",
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
            "- {}/{} (dir: {}/, access: {}, default branch: {}, base branch: {}, work branch: {})\n",
            repo.owner, repo.name, repo.dir, repo.access, repo.default_branch, repo.base_branch, branch
        ));
    }
    text.push_str(&format!(
        "Role: {}  Queue: {}  Action: {}  Kind: {}\n",
        context.work_item.role, context.work_item.queue, context.action, context.work_item.kind
    ));
    text.push_str(&format!("Target: {}\n", context.work_item.target));
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
