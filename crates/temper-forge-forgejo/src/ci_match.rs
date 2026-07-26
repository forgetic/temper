// SPDX-License-Identifier: MPL-2.0
//! Run matching for the Forgejo CI adapter.
//!
//! Adapts Forgejo Actions runs to a query target (a pull request and/or commit
//! SHA), mirroring the matching rules, PR-number derivation, and newest-first
//! sorting of the reference TypeScript tooling. An explicit query commit is a
//! mandatory ownership filter. A run carrying a different PR identity is still
//! rejected, while runs without PR identity (such as push workflows) remain
//! eligible for combined PR-and-commit reads.

use crate::types::ActionRunDto;
use chrono::{DateTime, Utc};
use serde_json::Value;
use temper_forge_model::PullRequestId;

/// Why a run was considered a match for a query target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MatchReason {
    /// `prettyref`/`head_branch` equals `#<pr>`.
    PrRef,
    /// The run head/commit SHA matches the target SHA.
    HeadSha,
    /// The event payload's pull-request number matches.
    EventPayloadNumber,
    /// The event payload's pull-request head SHA matches.
    EventPayloadHeadSha,
    /// The run branch/pretty ref matches the PR head ref.
    HeadBranch,
}

/// Resolved matching context for a query.
#[derive(Clone, Debug, Default)]
pub(crate) struct Target {
    pub pr_id: Option<PullRequestId>,
    pub pr_number: Option<u64>,
    pub pr_head_sha: Option<String>,
    pub pr_head_ref: Option<String>,
    pub commit_sha: Option<String>,
}

impl Target {
    /// The caller-supplied commit filter, when it is non-empty.
    pub(crate) fn explicit_commit(&self) -> Option<&str> {
        self.commit_sha.as_deref().filter(|sha| !sha.is_empty())
    }

    /// Whether any filter is active; an empty target matches every run.
    pub(crate) fn has_filter(&self) -> bool {
        self.pr_number.is_some()
            || self.explicit_commit().is_some()
            || self
                .pr_head_sha
                .as_deref()
                .is_some_and(|sha| !sha.is_empty())
    }
}

/// Decides whether a run matches a query target, returning the first reason.
pub(crate) fn match_run(run: &ActionRunDto, target: &Target) -> Option<MatchReason> {
    // A query commit is authoritative. PR refs/numbers/branches and the fetched
    // PR head are useful for PR-only history, but cannot prove that a run owns
    // this particular commit. When both sides do identify a PR, however, a
    // mismatch is conclusive: the same commit may back more than one PR and a
    // terminal job from one must not satisfy or fail another. Runs without a PR
    // identity remain eligible so push-based PR CI is preserved.
    if let Some(commit) = target.explicit_commit() {
        if let (Some(target_pr), Some(run_pr)) = (target.pr_number, run_pr_number(run)) {
            if target_pr != run_pr {
                return None;
            }
        }
        if sha_matches(&run.head_sha, commit) || sha_matches(&run.commit_sha, commit) {
            return Some(MatchReason::HeadSha);
        }
        if payload_pr_head_sha(run).is_some_and(|sha| sha_matches(&sha, commit)) {
            return Some(MatchReason::EventPayloadHeadSha);
        }
        return None;
    }

    if let Some(number) = target.pr_number {
        let tag = format!("#{number}");
        if run.prettyref == tag || run.head_branch == tag {
            return Some(MatchReason::PrRef);
        }
    }
    if let Some(sha) = target.pr_head_sha.as_deref().filter(|sha| !sha.is_empty()) {
        if sha_matches(&run.head_sha, sha) || sha_matches(&run.commit_sha, sha) {
            return Some(MatchReason::HeadSha);
        }
    }
    if let Some(number) = target.pr_number {
        if payload_pr_number(run) == Some(number) {
            return Some(MatchReason::EventPayloadNumber);
        }
    }
    if let Some(sha) = target.pr_head_sha.as_deref().filter(|sha| !sha.is_empty()) {
        if payload_pr_head_sha(run).is_some_and(|payload_sha| sha_matches(&payload_sha, sha)) {
            return Some(MatchReason::EventPayloadHeadSha);
        }
    }
    if let Some(head_ref) = target.pr_head_ref.as_deref() {
        if !head_ref.is_empty() && (run.head_branch == head_ref || run.prettyref == head_ref) {
            return Some(MatchReason::HeadBranch);
        }
    }
    None
}

/// Derives a PR number from a run via ref or event payload.
pub(crate) fn run_pr_number(run: &ActionRunDto) -> Option<u64> {
    if let Some(number) = parse_hash_ref(&run.prettyref) {
        return Some(number);
    }
    if let Some(number) = parse_hash_ref(&run.head_branch) {
        return Some(number);
    }
    payload_pr_number(run)
}

fn parse_hash_ref(value: &str) -> Option<u64> {
    value.strip_prefix('#').and_then(|rest| rest.parse().ok())
}

fn event_payload(run: &ActionRunDto) -> Option<Value> {
    if run.event_payload.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&run.event_payload).ok()
}

fn payload_pr_number(run: &ActionRunDto) -> Option<u64> {
    let payload = event_payload(run)?;
    if let Some(number) = payload.get("pull_request").and_then(|pr| pr.get("number")) {
        return number.as_u64();
    }
    payload.get("number").and_then(Value::as_u64)
}

pub(crate) fn payload_pr_head_sha(run: &ActionRunDto) -> Option<String> {
    let payload = event_payload(run)?;
    payload
        .get("pull_request")
        .and_then(|pr| pr.get("head"))
        .and_then(|head| head.get("sha"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Compares two SHAs, allowing safe short/full prefix matches (min 7 chars).
pub(crate) fn sha_matches(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    if left == right {
        return true;
    }
    let min = left.len().min(right.len());
    if min < 7 {
        return false;
    }
    left[..min] == right[..min]
}

/// Sorts runs newest first: created desc, then updated desc, then provider id desc.
pub(crate) fn sort_runs(runs: &mut [ActionRunDto]) {
    runs.sort_by(|a, b| {
        run_created(b)
            .cmp(&run_created(a))
            .then(run_updated(b).cmp(&run_updated(a)))
            .then(b.id.cmp(&a.id))
    });
}

pub(crate) fn run_created(run: &ActionRunDto) -> Option<DateTime<Utc>> {
    run.created_at.or(run.created)
}

pub(crate) fn run_updated(run: &ActionRunDto) -> Option<DateTime<Utc>> {
    run.updated_at.or(run.updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(prettyref: &str, head_branch: &str, head_sha: &str, event: &str) -> ActionRunDto {
        ActionRunDto {
            status: "success".to_string(),
            event: event.to_string(),
            prettyref: prettyref.to_string(),
            head_branch: head_branch.to_string(),
            head_sha: head_sha.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn matches_pr_ref() {
        let target = Target {
            pr_number: Some(7),
            ..Default::default()
        };
        let by_prettyref = run("#7", "feature", "abc", "push");
        assert_eq!(match_run(&by_prettyref, &target), Some(MatchReason::PrRef));
        let by_branch = run("main", "#7", "abc", "push");
        assert_eq!(match_run(&by_branch, &target), Some(MatchReason::PrRef));
    }

    #[test]
    fn matches_head_sha() {
        let target = Target {
            commit_sha: Some("0123456789abcdef".to_string()),
            ..Default::default()
        };
        let run = run("main", "main", "0123456789abcdef", "push");
        assert_eq!(match_run(&run, &target), Some(MatchReason::HeadSha));
    }

    #[test]
    fn combined_pr_and_commit_rejects_old_pr_and_branch_runs() {
        let target = Target {
            pr_number: Some(7),
            pr_head_sha: Some("current1234567".to_string()),
            pr_head_ref: Some("feature".to_string()),
            commit_sha: Some("current1234567".to_string()),
            ..Default::default()
        };

        let old_pr = run("#7", "feature", "oldhead1234567", "pull_request");
        assert_eq!(match_run(&old_pr, &target), None);

        let other_pr_same_head = run("#8", "feature", "current1234567", "pull_request");
        assert_eq!(
            match_run(&other_pr_same_head, &target),
            None,
            "a shared commit does not make another PR's jobs current"
        );

        let mut old_payload = run("main", "feature", "", "pull_request");
        old_payload.event_payload =
            r#"{"pull_request":{"number":7,"head":{"sha":"oldhead1234567"}}}"#.to_string();
        assert_eq!(match_run(&old_payload, &target), None);

        let current_push = run("feature", "feature", "current1234567", "push");
        assert_eq!(
            match_run(&current_push, &target),
            Some(MatchReason::HeadSha)
        );
    }

    #[test]
    fn explicit_commit_requires_provider_sha_evidence() {
        let target = Target {
            pr_number: Some(7),
            pr_head_sha: Some("current1234567".to_string()),
            commit_sha: Some("current1234567".to_string()),
            ..Default::default()
        };
        let no_sha = run("#7", "feature", "", "pull_request");
        assert_eq!(match_run(&no_sha, &target), None);
    }

    #[test]
    fn matches_event_payload_number_and_sha() {
        let mut run = run("main", "main", "zzz", "push");
        run.event_payload =
            "{\"pull_request\":{\"number\":42,\"head\":{\"sha\":\"deadbeefcafe\"}}}".to_string();
        let by_number = Target {
            pr_number: Some(42),
            ..Default::default()
        };
        assert_eq!(
            match_run(&run, &by_number),
            Some(MatchReason::EventPayloadNumber)
        );
        let by_sha = Target {
            commit_sha: Some("deadbeefcafe".to_string()),
            ..Default::default()
        };
        assert_eq!(
            match_run(&run, &by_sha),
            Some(MatchReason::EventPayloadHeadSha)
        );
    }

    #[test]
    fn branch_match_includes_push_runs_for_pr_heads() {
        let target = Target {
            pr_head_ref: Some("feature".to_string()),
            ..Default::default()
        };
        let pr_event = run("main", "feature", "abc", "pull_request");
        assert_eq!(match_run(&pr_event, &target), Some(MatchReason::HeadBranch));
        let push_event = run("main", "feature", "abc", "push");
        assert_eq!(
            match_run(&push_event, &target),
            Some(MatchReason::HeadBranch)
        );
        let pretty_ref_only = run("feature", "", "abc", "push");
        assert_eq!(
            match_run(&pretty_ref_only, &target),
            Some(MatchReason::HeadBranch)
        );
    }

    #[test]
    fn empty_target_has_no_filter() {
        assert!(!Target::default().has_filter());
        assert!(
            Target {
                pr_number: Some(1),
                ..Default::default()
            }
            .has_filter()
        );
    }

    #[test]
    fn derives_pr_number_from_ref_then_payload() {
        let by_ref = run("#7", "feature", "abc", "push");
        assert_eq!(run_pr_number(&by_ref), Some(7));
        let mut by_payload = run("main", "main", "abc", "push");
        by_payload.event_payload = "{\"number\":99}".to_string();
        assert_eq!(run_pr_number(&by_payload), Some(99));
        let none = run("main", "main", "abc", "push");
        assert_eq!(run_pr_number(&none), None);
    }
}
