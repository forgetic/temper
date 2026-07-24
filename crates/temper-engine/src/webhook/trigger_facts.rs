// SPDX-License-Identifier: MPL-2.0

//! Pure extraction of the §7 `trigger:` event facts from a verified webhook body.
//!
//! The webhook receiver ([`super`]) verifies and parses each delivery into a
//! [`ChangeHint`], then this module pulls the extra human-facing fields the §7
//! `trigger:` info lines carry — the issue author/title, the CI conclusion and
//! duration — straight from the same JSON. Keeping the parse here (and pure: no
//! `tracing`, no I/O) makes the §7 field contract unit-testable line-by-line and
//! keeps the receiver module focused on the verify/dispatch flow.

use chrono::{DateTime, Utc};
use serde_json::Value;
use temper_forge::{ChangeKind, HintArtifactKind};

use super::{ChangeHint, payload::ci_pr_number, payload::json_u64};

/// The §7 `trigger:` info events a single verified webhook delivery yields.
///
/// `issue.opened` and `ci.completed` are emitted once per accepted delivery (they
/// describe the inbound forge fact); `wake.received` is emitted per item the wake
/// scan actually enqueues (see [`super::run_wake_scan`]). These are
/// operator-facing `info` lines — the per-delivery debug accounting (`webhook
/// accepted …`) stays at `debug`. All three are projections of the same parsed
/// body, so the text and the structured fields cannot disagree (§3 rule 3).
///
/// Best-effort: a field the provider omits collapses to a neutral default rather
/// than dropping the whole line.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WebhookTriggerFacts {
    /// `issue.opened`: present when the delivery is an issue `opened` action.
    pub(super) issue_opened: Option<IssueOpenedFacts>,
    /// `ci.completed`: present when the delivery is a CI status event for a PR.
    pub(crate) ci_completed: Option<CiCompletedFacts>,
}

/// The fields the §7 `issue.opened` trigger line carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct IssueOpenedFacts {
    pub(super) number: u64,
    pub(super) author: String,
    pub(super) title: String,
}

/// The fields the §7 `ci.completed` trigger line carries.
///
/// `duration_ms` is best-effort: computed from the payload's start/finish
/// timestamps when both are present, then from a provider duration field when
/// available, else `0` (the numeric field stays numeric, and the human renderer
/// shows `0s`). The webhook body does not reliably carry a CI duration across
/// all of Forgejo's CI event shapes; a precise figure would require a follow-up
/// forge read, which the trigger plane deliberately avoids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CiCompletedFacts {
    pub(crate) pr_number: u64,
    pub(crate) conclusion: String,
    pub(crate) duration_ms: u64,
    pub(crate) completed_at: Option<DateTime<Utc>>,
}

impl CiCompletedFacts {
    pub(crate) fn terminal_verdict(&self) -> Option<crate::CiTerminalVerdict> {
        match self.conclusion.as_str() {
            "success" | "passed" => Some(crate::CiTerminalVerdict::Passed),
            "failure" | "failed" => Some(crate::CiTerminalVerdict::Failed),
            "cancelled" | "canceled" | "interrupted" | "timed_out" | "timeout" | "runner_lost"
            | "startup_failure" | "action_required" | "neutral" | "skipped" | "unknown" => {
                Some(crate::CiTerminalVerdict::RecoveryRequired)
            }
            _ => None,
        }
    }
}

/// Parses the §7 trigger facts from a verified webhook body for one event.
///
/// Pure (no `tracing`, no I/O) so the parse is unit-testable line-by-line. The
/// `event` is the provider event header (`super::webhook_event`); the `hint`
/// supplies the already-parsed repository, typed target, and change kind so this
/// only adds the human-facing extras.
pub(crate) fn parse_trigger_facts(
    body: &[u8],
    event: &str,
    hint: &ChangeHint,
) -> WebhookTriggerFacts {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return WebhookTriggerFacts::default();
    };

    let mut facts = WebhookTriggerFacts::default();

    if matches!(event, "issues" | "issue")
        && value.pointer("/action").and_then(Value::as_str) == Some("opened")
    {
        if let Some(number) = value.pointer("/issue/number").and_then(Value::as_u64) {
            let author = value
                .pointer("/issue/user/login")
                .or_else(|| value.pointer("/issue/user/username"))
                .or_else(|| value.pointer("/sender/login"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let title = value
                .pointer("/issue/title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            facts.issue_opened = Some(IssueOpenedFacts {
                number,
                author,
                title,
            });
        }
    }

    if hint.change == ChangeKind::Ci {
        if let Some(pr_number) = ci_pr_number(&value) {
            let conclusion = ci_conclusion(&value).unwrap_or("unknown").to_string();
            let duration_ms = ci_duration_ms(&value).unwrap_or(0);
            let completed_at = ci_timestamp(
                &value,
                &[
                    "/completed_at",
                    "/updated_at",
                    "/workflow_run/updated_at",
                    "/run/stopped",
                ],
            );
            facts.ci_completed = Some(CiCompletedFacts {
                pr_number,
                conclusion,
                duration_ms,
                completed_at,
            });
        }
    }

    facts
}

/// Extracts the CI conclusion token from a status payload (`success`, `failure`,
/// …), trying the field names Forgejo uses across its CI event shapes.
fn ci_conclusion(value: &Value) -> Option<&str> {
    value
        .pointer("/conclusion")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/status").and_then(Value::as_str))
        .or_else(|| value.pointer("/state").and_then(Value::as_str))
        .or_else(|| value.pointer("/run/status").and_then(Value::as_str))
        .or_else(|| value.pointer("/action").and_then(Value::as_str))
        .or_else(|| {
            value
                .pointer("/workflow_run/conclusion")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .pointer("/check_run/conclusion")
                .and_then(Value::as_str)
        })
}

/// Computes the CI run duration in milliseconds from payload timestamps, or from
/// an explicit duration field when Forgejo provides one.
fn ci_duration_ms(value: &Value) -> Option<u64> {
    ci_duration_from_timestamps(value).or_else(|| {
        value
            .pointer("/run/duration_ms")
            .and_then(json_u64)
            .or_else(|| value.pointer("/run/duration").and_then(json_u64))
    })
}

fn ci_duration_from_timestamps(value: &Value) -> Option<u64> {
    let start = ci_timestamp(
        value,
        &[
            "/started_at",
            "/run_started_at",
            "/workflow_run/run_started_at",
            "/run/started",
        ],
    )?;
    let finish = ci_timestamp(
        value,
        &[
            "/completed_at",
            "/updated_at",
            "/workflow_run/updated_at",
            "/run/stopped",
        ],
    )?;
    let delta = finish.signed_duration_since(start).num_milliseconds();
    u64::try_from(delta).ok()
}

/// Reads the first present RFC 3339 timestamp among the given JSON pointers.
fn ci_timestamp(value: &Value, pointers: &[&str]) -> Option<DateTime<Utc>> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|raw| raw.parse::<DateTime<Utc>>().ok())
    })
}

/// The workflow artifact kind a wake delivery feeds, for the §7 `artifact=` field.
pub(super) fn wake_artifact_kind(kind: HintArtifactKind) -> &'static str {
    match kind {
        HintArtifactKind::Issue => "intake",
        HintArtifactKind::PullRequest => "pull_request",
    }
}

/// The queue a wake delivery routes into, for the §7 `queue=` field.
pub(super) fn wake_queue(kind: HintArtifactKind, change: ChangeKind) -> &'static str {
    match (kind, change) {
        (HintArtifactKind::Issue, ChangeKind::Created) => "raw_intake",
        _ => "wake",
    }
}

#[cfg(test)]
mod tests {
    use temper_forge::{ItemNumber, RepositoryPath};

    use super::*;

    fn issue_hint(number: u64) -> ChangeHint {
        ChangeHint::issue(
            RepositoryPath::new("acme", "widgets"),
            ItemNumber::new(number),
            ChangeKind::Created,
        )
    }

    #[test]
    fn parses_issue_opened_author_and_title() {
        let body = serde_json::to_vec(&serde_json::json!({
            "action": "opened",
            "issue": {
                "number": 42,
                "title": "Cache invalidation drops stale keys on resize",
                "user": { "login": "alice" }
            }
        }))
        .expect("body serializes");

        let facts = parse_trigger_facts(&body, "issues", &issue_hint(42));
        assert_eq!(
            facts.issue_opened,
            Some(IssueOpenedFacts {
                number: 42,
                author: "alice".to_string(),
                title: "Cache invalidation drops stale keys on resize".to_string(),
            })
        );
        assert_eq!(facts.ci_completed, None);
    }

    #[test]
    fn ignores_non_opened_issue_actions() {
        let body = serde_json::to_vec(&serde_json::json!({
            "action": "edited",
            "issue": { "number": 42, "title": "x", "user": { "login": "alice" } }
        }))
        .expect("body serializes");

        let facts = parse_trigger_facts(&body, "issues", &issue_hint(42));
        assert_eq!(facts.issue_opened, None);
    }

    #[test]
    fn missing_issue_author_defaults_to_unknown() {
        let body = serde_json::to_vec(&serde_json::json!({
            "action": "opened",
            "issue": { "number": 7, "title": "No author here" }
        }))
        .expect("body serializes");

        let facts = parse_trigger_facts(&body, "issues", &issue_hint(7));
        assert_eq!(
            facts.issue_opened,
            Some(IssueOpenedFacts {
                number: 7,
                author: "unknown".to_string(),
                title: "No author here".to_string(),
            })
        );
    }

    #[test]
    fn parses_ci_completed_conclusion_and_duration() {
        let body = serde_json::to_vec(&serde_json::json!({
            "status": "success",
            "started_at": "2026-06-16T10:19:10Z",
            "completed_at": "2026-06-16T10:23:50Z",
            "pull_request": { "number": 44 }
        }))
        .expect("body serializes");

        let hint = ChangeHint::pull_request(
            RepositoryPath::new("acme", "widgets"),
            ItemNumber::new(44),
            ChangeKind::Ci,
        );
        let facts = parse_trigger_facts(&body, "status", &hint);
        assert_eq!(
            facts.ci_completed,
            Some(CiCompletedFacts {
                pr_number: 44,
                conclusion: "success".to_string(),
                // 4m40s == 280_000 ms.
                duration_ms: 280_000,
                completed_at: Some("2026-06-16T10:23:50Z".parse().unwrap()),
            })
        );
        assert_eq!(facts.issue_opened, None);
    }

    #[test]
    fn parses_forgejo_action_run_success_ci_completed() {
        let event_payload = serde_json::json!({
            "pull_request": { "number": 23 }
        })
        .to_string();
        let body = serde_json::to_vec(&serde_json::json!({
            "action": "success",
            "run": {
                "status": "success",
                "started": "2026-06-29T16:48:49+01:00",
                "stopped": "2026-06-29T16:51:00+01:00",
                "event_payload": event_payload
            }
        }))
        .expect("body serializes");

        let hint = ChangeHint::pull_request(
            RepositoryPath::new("ai", "temper"),
            ItemNumber::new(23),
            ChangeKind::Ci,
        );
        let facts = parse_trigger_facts(&body, "action_run_success", &hint);
        assert_eq!(
            facts.ci_completed,
            Some(CiCompletedFacts {
                pr_number: 23,
                conclusion: "success".to_string(),
                duration_ms: 131_000,
                completed_at: Some("2026-06-29T15:51:00Z".parse().unwrap()),
            })
        );
        assert_eq!(facts.issue_opened, None);
    }

    #[test]
    fn ci_without_pr_number_yields_no_ci_line() {
        let body = serde_json::to_vec(&serde_json::json!({
            "status": "success",
            "started_at": "2026-06-16T10:19:10Z",
            "completed_at": "2026-06-16T10:23:50Z"
        }))
        .expect("body serializes");

        let hint = ChangeHint::repository(RepositoryPath::new("acme", "widgets"), ChangeKind::Ci);
        let facts = parse_trigger_facts(&body, "status", &hint);
        assert_eq!(facts.ci_completed, None);
    }

    #[test]
    fn ci_without_timestamps_reports_zero_duration() {
        let body = serde_json::to_vec(&serde_json::json!({
            "conclusion": "failure",
            "pull_request": { "number": 19 }
        }))
        .expect("body serializes");

        let hint = ChangeHint::pull_request(
            RepositoryPath::new("acme", "api"),
            ItemNumber::new(19),
            ChangeKind::Ci,
        );
        let facts = parse_trigger_facts(&body, "status", &hint);
        assert_eq!(
            facts.ci_completed,
            Some(CiCompletedFacts {
                pr_number: 19,
                conclusion: "failure".to_string(),
                duration_ms: 0,
                completed_at: None,
            })
        );
    }

    #[test]
    fn webhook_terminal_categories_do_not_turn_interruptions_into_failures() {
        let facts = |conclusion: &str| CiCompletedFacts {
            pr_number: 19,
            conclusion: conclusion.to_string(),
            duration_ms: 0,
            completed_at: None,
        };

        assert_eq!(
            facts("failure").terminal_verdict(),
            Some(crate::CiTerminalVerdict::Failed)
        );
        for conclusion in [
            "cancelled",
            "interrupted",
            "timed_out",
            "runner_lost",
            "startup_failure",
            "action_required",
            "neutral",
            "skipped",
            "unknown",
        ] {
            assert_eq!(
                facts(conclusion).terminal_verdict(),
                Some(crate::CiTerminalVerdict::RecoveryRequired),
                "{conclusion}"
            );
        }
    }

    #[test]
    fn malformed_body_parses_to_empty_facts() {
        let facts = parse_trigger_facts(b"not json", "issues", &issue_hint(1));
        assert_eq!(facts, WebhookTriggerFacts::default());
    }

    #[test]
    fn wake_artifact_and_queue_map_issue_to_intake() {
        assert_eq!(wake_artifact_kind(HintArtifactKind::Issue), "intake");
        assert_eq!(
            wake_queue(HintArtifactKind::Issue, ChangeKind::Created),
            "raw_intake"
        );
        assert_eq!(
            wake_artifact_kind(HintArtifactKind::PullRequest),
            "pull_request"
        );
        assert_eq!(
            wake_queue(HintArtifactKind::PullRequest, ChangeKind::Review),
            "wake"
        );
    }
}
