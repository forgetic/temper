//! Human-readable attempt events and work-item attribution.
use super::*;

/// The §7 run-kind (`role/kind`) for a worker role.
///
/// The human renderer formats `role/kind` (e.g. `architect/triage`,
/// `engineer/coding`). We map the role's *activity verb* here: architect
/// triages, the engineer codes, the reviewer reviews. Unknown roles fall back
/// to a neutral `run` so the line still reads sensibly.
pub(super) fn run_kind(role: &str) -> &'static str {
    match role {
        "architect" => "triage",
        "engineer" => "coding",
        "reviewer" => "review",
        _ => "run",
    }
}

/// The one-line `detail` for the `agent.started` line, per §7's examples.
pub(super) fn started_detail(kind: &str) -> &'static str {
    match kind {
        "triage" => "reading issue + repo context",
        "coding" => "preparing workspace, implementing",
        "review" => "reviewing changes",
        _ => "running",
    }
}

/// The token-totals suffix appended to a successful `agent.finished` summary.
///
/// Renders `<input> in / <output> out, <N> tool calls` with the token counts
/// humanized via [`human_count`] (the §7 example: `470k in / 6.4k out, 52 tool
/// calls`). The tool-call count is kept raw — it is a small cardinal number, not
/// a token volume — so a one-off run reads `1 tool call`.
pub(super) fn totals_suffix(totals: RunTotals) -> String {
    let calls = totals.tool_calls;
    let unit = if calls == 1 {
        "tool call"
    } else {
        "tool calls"
    };
    format!(
        "{} in / {} out, {calls} {unit}",
        human_count(totals.input),
        human_count(totals.output),
    )
}

/// Humanizes a token count with a `k` suffix above 1000.
///
/// Under 1000 the raw integer is shown (`0`, `999`). At/above 1000 the value is
/// expressed in thousands: 1000–9999 keep one decimal of precision (`6379` ->
/// `6.4k`), 10_000 and up round to a whole `k` (`470306` -> `470k`). A trailing
/// `.0` is dropped so a round value stays tight (`2000` -> `2k`). Arithmetic is
/// integer-only (half-up rounding) to avoid float surprises.
pub(super) fn human_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n >= 10_000 {
        // Round to the nearest whole thousand: floor((n + 500) / 1000).
        let thousands = (n + 500) / 1000;
        return format!("{thousands}k");
    }
    // 1000–9999: one decimal, i.e. tenths = round(n / 100) half-up.
    let tenths = (n + 50) / 100;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{frac}k")
    }
}

/// Builds the §7 work-item subject reference from the workspace context.
///
/// `repo` is the primary repository's bare `owner/name` path; the number and
/// issue-vs-PR kind come from the work item. Returns `None` when there is no
/// primary repo or the target number cannot be parsed — in that case the agent
/// events are skipped rather than logged against a bogus `#0` ref.
pub(super) fn work_item_ref(context: &WorkspaceContext) -> Option<WorkItemRef> {
    let repo = context.primary()?;
    let repo_path = format!("{}/{}", repo.owner, repo.name);
    let number = parse_target_number(&context.work_item.target)?;
    let is_pr = context.work_item.kind == "pull_request";
    Some(if is_pr {
        WorkItemRef::pull_request(repo_path, number)
    } else {
        WorkItemRef::issue(repo_path, number)
    })
}

/// Extracts the artifact number from a Debug-formatted target string.
///
/// The worker builds `target` as e.g. `Issue { number: ItemNumber(7) }` or
/// `PullRequest { number: ItemNumber(44) }`
/// ([`temper_worker`]'s context assembly). For robustness this also accepts the
/// bare `number: 7` form some fixtures use. Returns `None` if no `number:`
/// segment with a parseable integer is found, so the caller can skip the emit.
pub(super) fn parse_target_number(target: &str) -> Option<u64> {
    // Find the `number:` key, then take the first run of ASCII digits after it
    // (skipping the `ItemNumber(` wrapper if present).
    let after_key = target.split("number:").nth(1)?;
    let digits: String = after_key
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}
