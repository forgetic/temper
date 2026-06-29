// SPDX-License-Identifier: MPL-2.0
//! Run matching for the Forgejo CI adapter.
//!
//! Adapts Forgejo Actions runs to a query target (a pull request and/or commit
//! SHA), mirroring the matching rules, PR-number derivation, and newest-first
//! sorting of the reference TypeScript tooling: PR-ref match, head-SHA match,
//! event-payload PR number / head SHA match, and PR head-branch match (only for
//! pull-request events).

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
    /// The run head branch matches the PR head ref (pull-request events only).
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
    /// Candidate SHAs to match a run against, query commit first.
    fn candidate_shas(&self) -> Vec<&str> {
        let mut shas = Vec::new();
        if let Some(sha) = self.commit_sha.as_deref() {
            if !sha.is_empty() {
                shas.push(sha);
            }
        }
        if let Some(sha) = self.pr_head_sha.as_deref() {
            if !sha.is_empty() {
                shas.push(sha);
            }
        }
        shas
    }

    /// Whether any filter is active; an empty target matches every run.
    pub(crate) fn has_filter(&self) -> bool {
        self.pr_number.is_some() || !self.candidate_shas().is_empty()
    }
}

/// A run's repo-stable index: `index_in_repo`, then `run_number`, then `id`.
pub(crate) fn run_index(run: &ActionRunDto) -> u64 {
    if run.index_in_repo > 0 {
        run.index_in_repo
    } else if run.run_number > 0 {
        run.run_number
    } else {
        run.id
    }
}

/// Decides whether a run matches a query target, returning the first reason.
pub(crate) fn match_run(run: &ActionRunDto, target: &Target) -> Option<MatchReason> {
    if let Some(number) = target.pr_number {
        let tag = format!("#{number}");
        if run.prettyref == tag || run.head_branch == tag {
            return Some(MatchReason::PrRef);
        }
    }
    for sha in target.candidate_shas() {
        if sha_match(&run.head_sha, sha) || sha_match(&run.commit_sha, sha) {
            return Some(MatchReason::HeadSha);
        }
    }
    if let Some(number) = target.pr_number {
        if payload_pr_number(run) == Some(number) {
            return Some(MatchReason::EventPayloadNumber);
        }
    }
    for sha in target.candidate_shas() {
        if let Some(payload_sha) = payload_pr_head_sha(run) {
            if sha_match(&payload_sha, sha) {
                return Some(MatchReason::EventPayloadHeadSha);
            }
        }
    }
    if let Some(head_ref) = target.pr_head_ref.as_deref() {
        if !head_ref.is_empty() && is_pull_request_event(&run.event) && run.head_branch == head_ref
        {
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

fn is_pull_request_event(event: &str) -> bool {
    event.starts_with("pull_request")
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

fn payload_pr_head_sha(run: &ActionRunDto) -> Option<String> {
    let payload = event_payload(run)?;
    payload
        .get("pull_request")
        .and_then(|pr| pr.get("head"))
        .and_then(|head| head.get("sha"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Compares two SHAs, allowing short/long prefix matches (min 7 chars).
fn sha_match(left: &str, right: &str) -> bool {
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

/// Sorts runs newest first: created desc, then updated desc, then index desc.
pub(crate) fn sort_runs(runs: &mut [ActionRunDto]) {
    runs.sort_by(|a, b| {
        run_created(b)
            .cmp(&run_created(a))
            .then(run_updated(b).cmp(&run_updated(a)))
            .then(run_index(b).cmp(&run_index(a)))
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
            index_in_repo: 5,
            run_number: 5,
            status: "success".to_string(),
            event: event.to_string(),
            prettyref: prettyref.to_string(),
            head_branch: head_branch.to_string(),
            head_sha: head_sha.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn run_index_prefers_index_in_repo() {
        let mut run = run("#7", "feature", "abc", "push");
        assert_eq!(run_index(&run), 5);
        run.index_in_repo = 0;
        run.run_number = 9;
        assert_eq!(run_index(&run), 9);
        run.run_number = 0;
        run.id = 13;
        assert_eq!(run_index(&run), 13);
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
    fn branch_match_only_for_pull_request_events() {
        let target = Target {
            pr_head_ref: Some("feature".to_string()),
            ..Default::default()
        };
        let pr_event = run("main", "feature", "abc", "pull_request");
        assert_eq!(match_run(&pr_event, &target), Some(MatchReason::HeadBranch));
        let push_event = run("main", "feature", "abc", "push");
        assert_eq!(match_run(&push_event, &target), None);
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
