// SPDX-License-Identifier: MPL-2.0

//! Pure human-message renderers for the lifecycle events (§7 of the design).
//!
//! Each function here takes the same typed inputs the corresponding `emit_*`
//! constructor receives and returns the **human projection** — the exact ASCII
//! line from §7's reference output, *without* the service prefix (the prefix is
//! prepended by the emit site so the message column aligns; see
//! [`Service::human_prefix`]).
//!
//! These renderers are pure (no `tracing`, no I/O) precisely so the §7 contract
//! is unit-testable string-by-string. The `emit_*` functions in the parent
//! module call these to build the trailing positional message, and pass the same
//! inputs as structured fields — one emit site, two projections that cannot
//! disagree (§3 rule 3).
//!
//! ## Format rules (§7)
//!
//! - Pure ASCII: `->` for transitions/queue moves, `--` for separators/em-dash,
//!   `|` as the field separator, `[repo#n]` / `[repo PR#n]` as the subject tag.
//! - Numbers are formatted by the human renderer (durations via
//!   [`format_duration_ms`]); the machine fields stay numeric.
//!
//! [`Service::human_prefix`]: crate::Service::human_prefix
//! [`format_duration_ms`]: crate::format_duration_ms

use crate::WorkItemRef;
use crate::duration::format_duration_ms;
use crate::redact::redacted_preview;

use super::{AgentTerminalReasonV1, AgentTerminalStatus, ModelFailureV1};

/// Character bound applied to free-text previews (issue titles, summaries).
///
/// Generous enough to keep a normal title intact while capping a pathological
/// one. Matches the runner's historical 240-char preview budget.
pub(crate) const PREVIEW_LIMIT: usize = 240;

/// `[acme/widgets#42] issue opened by alice "title"`
pub(crate) fn issue_opened(item: &WorkItemRef, author: &str, title: &str) -> String {
    format!(
        "{} issue opened by {author} \"{}\"",
        item.human_tag(),
        redacted_preview(title, PREVIEW_LIMIT)
    )
}

/// `[acme/widgets#42] wake | artifact=intake queue=raw_intake`
pub(crate) fn wake_received(item: &WorkItemRef, artifact_kind: &str, queue: &str) -> String {
    format!(
        "{} wake | artifact={artifact_kind} queue={queue}",
        item.human_tag()
    )
}

/// `[acme/widgets PR#44] CI completed: success (4m40s)`
pub(crate) fn ci_completed(item: &WorkItemRef, conclusion: &str, duration_ms: u64) -> String {
    format!(
        "{} CI completed: {conclusion} ({})",
        item.human_tag(),
        format_duration_ms(duration_ms)
    )
}

/// `[acme/widgets#42] mechanical claimed lease (ttl 10m) | running mark_untriaged`
pub(crate) fn lease_claimed(
    item: &WorkItemRef,
    role: &str,
    ttl_human: &str,
    running: &str,
) -> String {
    format!(
        "{} {role} claimed lease (ttl {ttl_human}) | running {running}",
        item.human_tag()
    )
}

/// `[acme/widgets#42] architect lease released`
pub(crate) fn lease_released(item: &WorkItemRef, role: &str) -> String {
    format!("{} {role} lease released", item.human_tag())
}

/// `[acme/widgets#42] architect lease lost -- expired (held 11m)`
pub(crate) fn lease_lost(item: &WorkItemRef, role: &str, reason: &str) -> String {
    format!("{} {role} lease lost -- {reason}", item.human_tag())
}

/// `[acme/widgets#42] architect/triage start | reading issue + repo context`
pub(crate) fn agent_started(item: &WorkItemRef, role: &str, kind: &str, detail: &str) -> String {
    format!("{} {role}/{kind} start | {detail}", item.human_tag())
}

/// `[acme/widgets#42] architect/triage done in 1m13s | verdict=ready_code`
#[allow(clippy::too_many_arguments)] // Mirrors the typed lifecycle event fields.
pub(crate) fn agent_finished(
    item: &WorkItemRef,
    role: &str,
    kind: &str,
    status: AgentTerminalStatus,
    terminal_reason: Option<AgentTerminalReasonV1>,
    model_failure: Option<&ModelFailureV1>,
    duration_ms: u64,
    summary: &str,
) -> String {
    let detail = match (status, terminal_reason, model_failure) {
        (AgentTerminalStatus::Succeeded, _, _) => summary.to_string(),
        (
            AgentTerminalStatus::Failed | AgentTerminalStatus::Cancelled,
            Some(AgentTerminalReasonV1::ModelError),
            Some(failure),
        ) => model_failure_detail(failure),
        (AgentTerminalStatus::Failed | AgentTerminalStatus::Cancelled, reason, _) => reason
            .map(AgentTerminalReasonV1::as_str)
            .unwrap_or(summary)
            .to_string(),
    };
    format!(
        "{} {role}/{kind} {} in {} | {}",
        item.human_tag(),
        status.human_verb(),
        format_duration_ms(duration_ms),
        redacted_preview(&detail, PREVIEW_LIMIT)
    )
}

fn model_failure_detail(failure: &ModelFailureV1) -> String {
    let mut detail = format!(
        "model_error | {}/{} category={} retryable={}",
        failure.provider,
        failure.model,
        failure.category.as_str(),
        failure.retryable
    );
    if let Some(status) = failure.http_status {
        detail.push_str(&format!(" http_status={status}"));
    }
    if let Some(request_id) = &failure.provider_request_id {
        detail.push_str(&format!(" request_id={request_id}"));
    }
    if let Some(code) = &failure.provider_error_code {
        detail.push_str(&format!(" provider_code={code}"));
    }
    detail
}

/// `[acme/widgets#42] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready`
///
/// `detail` and `labels_delta` are optional trailing `| …` segments; either may
/// be empty, in which case its segment (and separator) is omitted.
pub(crate) fn transition_applied(
    item: &WorkItemRef,
    transition: &str,
    detail: &str,
    labels_delta: &str,
) -> String {
    let mut line = format!("{} {transition} applied", item.human_tag());
    if !detail.is_empty() {
        line.push_str(" | ");
        line.push_str(detail);
    }
    if !labels_delta.is_empty() {
        line.push_str(" | ");
        line.push_str(labels_delta);
    }
    line
}

/// `[acme/widgets#42] -> queue 'triage' | awaiting architect`
///
/// `note` is an optional trailing `| …` segment (e.g. `awaiting architect`);
/// empty omits it.
pub(crate) fn queue_entered(item: &WorkItemRef, queue: &str, note: &str) -> String {
    let mut line = format!("{} -> queue '{queue}'", item.human_tag());
    if !note.is_empty() {
        line.push_str(" | ");
        line.push_str(note);
    }
    line
}

/// `[acme/widgets PR#44] gates: ci_gate=pending dependency_gate=ok | waiting on CI`
///
/// `gates` is the pre-joined `name=state name=state` summary; `note` is the
/// trailing `| …` segment (e.g. `waiting on CI` or
/// `-> queue 'landing' eligible to land`).
pub(crate) fn gate_evaluated(item: &WorkItemRef, gates: &str, note: &str) -> String {
    let mut line = format!("{} gates: {gates}", item.human_tag());
    if !note.is_empty() {
        line.push_str(" | ");
        line.push_str(note);
    }
    line
}

/// `[acme/widgets PR#44] opened "title" | implementation, for #42`
pub(crate) fn pr_opened(item: &WorkItemRef, title: &str, kind: &str, for_issue: u64) -> String {
    format!(
        "{} opened \"{}\" | {kind}, for #{for_issue}",
        item.human_tag(),
        redacted_preview(title, PREVIEW_LIMIT)
    )
}

/// `[acme/widgets PR#44] refreshed "title" | implementation, for #42`
pub(crate) fn pr_updated(
    item: &WorkItemRef,
    title: &str,
    kind: &str,
    for_issue: u64,
    action: &str,
) -> String {
    let action = if action.trim().is_empty() {
        "updated"
    } else {
        action.trim()
    };
    format!(
        "{} {action} \"{}\" | {kind}, for #{for_issue}",
        item.human_tag(),
        redacted_preview(title, PREVIEW_LIMIT)
    )
}

/// `[acme/widgets PR#44] merged -> main (squash e3f9a1c) | +landed`
pub(crate) fn pr_merged(
    item: &WorkItemRef,
    target_branch: &str,
    merge_method: &str,
    commit: &str,
    labels_delta: &str,
) -> String {
    let mut line = format!(
        "{} merged -> {target_branch} ({merge_method} {commit})",
        item.human_tag()
    );
    if !labels_delta.is_empty() {
        line.push_str(" | ");
        line.push_str(labels_delta);
    }
    line
}

/// `[acme/widgets#42] resolved -- implemented by PR#44 | intake -> landed in 9m19s`
pub(crate) fn item_resolved(
    item: &WorkItemRef,
    resolution: &str,
    from_state: &str,
    to_state: &str,
    duration_ms: u64,
) -> String {
    format!(
        "{} resolved -- {resolution} | {from_state} -> {to_state} in {}",
        item.human_tag(),
        format_duration_ms(duration_ms)
    )
}

/// `architect busy (concurrency=1) | 2 queued: acme/api#7, acme/widgets#43`
///
/// A *global* line about the shared resource: it carries no subject `[repo#n]`
/// tag (§7), but names the waiting items so they stay greppable. `waiting` is
/// the already-joined list of `artifact.ref` strings.
pub(crate) fn role_saturated(role: &str, concurrency: u64, waiting: &[String]) -> String {
    if waiting.is_empty() {
        return format!("{role} busy (concurrency={concurrency})");
    }
    let count = waiting.len();
    format!(
        "{role} busy (concurrency={concurrency}) | {count} queued: {}",
        waiting.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue42() -> WorkItemRef {
        WorkItemRef::issue("acme/widgets", 42)
    }

    fn pr44() -> WorkItemRef {
        WorkItemRef::pull_request("acme/widgets", 44)
    }

    #[test]
    fn trigger_lines_match_spec() {
        assert_eq!(
            issue_opened(
                &issue42(),
                "alice",
                "Cache invalidation drops stale keys on resize"
            ),
            "[acme/widgets#42] issue opened by alice \"Cache invalidation drops stale keys on resize\""
        );
        assert_eq!(
            wake_received(&issue42(), "intake", "raw_intake"),
            "[acme/widgets#42] wake | artifact=intake queue=raw_intake"
        );
        assert_eq!(
            ci_completed(&pr44(), "success", 280_000),
            "[acme/widgets PR#44] CI completed: success (4m40s)"
        );
    }

    #[test]
    fn worker_lease_lines_match_spec() {
        assert_eq!(
            lease_claimed(
                &WorkItemRef::issue("acme/widgets", 42),
                "mechanical",
                "10m",
                "mark_untriaged"
            ),
            "[acme/widgets#42] mechanical claimed lease (ttl 10m) | running mark_untriaged"
        );
        assert_eq!(
            lease_released(&issue42(), "architect"),
            "[acme/widgets#42] architect lease released"
        );
        assert_eq!(
            lease_lost(&issue42(), "architect", "expired (held 11m)"),
            "[acme/widgets#42] architect lease lost -- expired (held 11m)"
        );
    }

    #[test]
    fn agent_lines_match_spec() {
        assert_eq!(
            agent_started(
                &issue42(),
                "architect",
                "triage",
                "reading issue + repo context"
            ),
            "[acme/widgets#42] architect/triage start | reading issue + repo context"
        );
        assert_eq!(
            agent_finished(
                &issue42(),
                "architect",
                "triage",
                AgentTerminalStatus::Succeeded,
                Some(AgentTerminalReasonV1::Completed),
                None,
                73_000,
                "verdict=ready_code"
            ),
            "[acme/widgets#42] architect/triage done in 1m13s | verdict=ready_code"
        );
        assert_eq!(
            agent_finished(
                &issue42(),
                "engineer",
                "coding",
                AgentTerminalStatus::Failed,
                Some(AgentTerminalReasonV1::BudgetExhausted),
                None,
                73_000,
                "agent exceeded its tool budget"
            ),
            "[acme/widgets#42] engineer/coding failed in 1m13s | budget_exhausted"
        );
        assert_eq!(
            agent_finished(
                &issue42(),
                "reviewer",
                "review",
                AgentTerminalStatus::Cancelled,
                Some(AgentTerminalReasonV1::Aborted),
                None,
                73_000,
                "worker cancelled the run"
            ),
            "[acme/widgets#42] reviewer/review cancelled in 1m13s | aborted"
        );
    }

    #[test]
    fn engine_lines_match_spec() {
        assert_eq!(
            transition_applied(
                &issue42(),
                "triage_intake_to_code",
                "body rewritten as code spec",
                "-untriaged +code +ready"
            ),
            "[acme/widgets#42] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready"
        );
        assert_eq!(
            queue_entered(&issue42(), "triage", "awaiting architect"),
            "[acme/widgets#42] -> queue 'triage' | awaiting architect"
        );
        assert_eq!(
            gate_evaluated(
                &pr44(),
                "ci_gate=pending dependency_gate=ok",
                "waiting on CI"
            ),
            "[acme/widgets PR#44] gates: ci_gate=pending dependency_gate=ok | waiting on CI"
        );
        assert_eq!(
            pr_opened(
                &pr44(),
                "Fix cache invalidation on resize",
                "implementation",
                42
            ),
            "[acme/widgets PR#44] opened \"Fix cache invalidation on resize\" | implementation, for #42"
        );
        assert_eq!(
            pr_updated(
                &pr44(),
                "Fix cache invalidation on resize",
                "implementation",
                42,
                "refreshed"
            ),
            "[acme/widgets PR#44] refreshed \"Fix cache invalidation on resize\" | implementation, for #42"
        );
        assert_eq!(
            pr_merged(&pr44(), "main", "squash", "e3f9a1c", "+landed"),
            "[acme/widgets PR#44] merged -> main (squash e3f9a1c) | +landed"
        );
        assert_eq!(
            item_resolved(
                &issue42(),
                "implemented by PR#44",
                "intake",
                "landed",
                559_000
            ),
            "[acme/widgets#42] resolved -- implemented by PR#44 | intake -> landed in 9m19s"
        );
    }

    #[test]
    fn role_saturated_has_no_subject_tag_but_names_waiters() {
        assert_eq!(
            role_saturated(
                "architect",
                1,
                &["acme/api#7".to_string(), "acme/widgets#43".to_string()]
            ),
            "architect busy (concurrency=1) | 2 queued: acme/api#7, acme/widgets#43"
        );
        // Single waiter.
        assert_eq!(
            role_saturated("engineer", 1, &["acme/widgets#42".to_string()]),
            "engineer busy (concurrency=1) | 1 queued: acme/widgets#42"
        );
        // Saturated with nothing yet queued.
        assert_eq!(
            role_saturated("architect", 1, &[]),
            "architect busy (concurrency=1)"
        );
    }

    #[test]
    fn optional_segments_are_omitted_when_empty() {
        assert_eq!(
            transition_applied(&issue42(), "mark_untriaged", "", "+untriaged"),
            "[acme/widgets#42] mark_untriaged applied | +untriaged"
        );
        assert_eq!(
            queue_entered(&issue42(), "code_ready", ""),
            "[acme/widgets#42] -> queue 'code_ready'"
        );
    }

    #[test]
    fn free_text_previews_are_redacted() {
        // A title carrying a credential collapses to the marker.
        assert_eq!(
            issue_opened(&issue42(), "mallory", "token=abcd leaked here"),
            "[acme/widgets#42] issue opened by mallory \"<redacted>\""
        );
    }
}
