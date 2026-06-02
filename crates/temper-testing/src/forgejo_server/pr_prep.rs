//! Forgejo-only pull-request prep: make a PR's head branch real before opening.
//!
//! # Why this exists
//!
//! The deterministic fake engineer opens a PR with head branch
//! `fake/pr-for-code-{N}` (`crate::pull_request_input`,
//! `crate::agents::implementation_pr_input`). On the filesystem/memory backends
//! that head is never a real git ref — nothing checks. On **real Forgejo**,
//! `create_pull_request` against a non-existent head fails with
//! `HTTP 404 "The target couldn't be found."` (Phase 0 spike, findings §1). A PR
//! cannot be opened until its head branch exists with a commit that differs from
//! base.
//!
//! # The seam (findings §1, option (a) — preferred)
//!
//! This is a **Forgejo-specific PR-prep step the worker runs before opening a
//! PR**, not a change to the backend-agnostic `Forge` trait. It derives the head
//! branch from the same [`CreatePullRequest::source`] the agent already sets and
//! the base from [`CreatePullRequest::target`], creates the head branch off base
//! (`POST …/branches`), and writes one trivial differing file
//! (`POST …/contents/{path}`). Then a subsequent `create_pull_request` against
//! the real server succeeds and the PR is `mergeable`.
//!
//! It lives in `temper-testing` (test-fixture glue), keyed off the
//! `CreatePullRequest` value, and is invoked **only** on the Forgejo backend. The
//! filesystem backend never calls it and keeps working unchanged.
//!
//! # Idempotency
//!
//! A worker tick that re-attempts the same PR must not error on an already-prepped
//! head. Both calls tolerate the "already exists" conflict: re-creating the
//! branch (`409/422`) and re-committing the prep file (`422`) are treated as
//! "already prepped" and succeed. The prep is therefore safe to run on every
//! `open_pull_request` attempt.

use super::provision::{self, ProvisionError, Result};
use super::provision_rest as rest;
use temper_forge::CreatePullRequest;

/// The directory the trivial prep file lives in, off the repo root. Kept out of
/// the way of the CI sentinel (`ci-ok`) and the workflow file.
const PREP_DIR: &str = ".temper-pr-prep";

/// The path of the trivial file the CI-pass commit writes. Its content is
/// irrelevant to the gate (which reads the **commit message**, see
/// [`provision::CI_PASS_MARKER`]); the file only exists to make a non-empty,
/// uniquely-named commit. A per-call suffix keeps repeated pass commits distinct.
const CI_SENTINEL_DIR: &str = ".temper-ci";

/// Ensures the head branch and a trivial differing commit for `input` exist.
///
/// Derives the head branch from `input.source.branch` and the base from
/// `input.target.branch` — exactly the branches the agent set on the
/// `CreatePullRequest`. Creates the head off base, then commits one trivial file
/// unique to the head so the diff against base is non-empty.
///
/// `token` is any token authorized to write to the repo (a role token or the
/// admin token); `owner`/`name` are the repository coordinates. Idempotent: a
/// re-run against an already-prepped head is a no-op success.
///
/// This is the Forgejo-only seam; see the module docs for why it is not on the
/// `Forge` trait.
pub async fn prepare_pull_request_head(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    input: &CreatePullRequest,
) -> Result<()> {
    let head = input.source.branch.as_str();
    let base_branch = input.target.branch.as_str();
    if head.is_empty() {
        return Err(ProvisionError::Shape {
            what: "pr-prep head branch".into(),
            detail: "CreatePullRequest.source.branch is empty".into(),
        });
    }
    if base_branch.is_empty() {
        return Err(ProvisionError::Shape {
            what: "pr-prep base branch".into(),
            detail: "CreatePullRequest.target.branch is empty".into(),
        });
    }

    let client = rest::http_client()?;

    // 1. Create the head branch off base. Tolerates "already exists".
    rest::create_branch(&client, base_url, token, owner, name, head, base_branch).await?;

    // 2. Write one trivial file unique to this head so the branch diverges from
    //    base with a non-empty diff. The path is derived from the head branch so
    //    distinct PRs never collide, and a re-commit of the same path is the
    //    tolerated "already exists" case (idempotent).
    let path = prep_file_path(head);
    rest::commit_file(
        &client,
        base_url,
        token,
        owner,
        name,
        &path,
        &prep_file_contents(head),
        &format!("prep PR head {head}"),
        head,
    )
    .await
}

/// Idempotently pushes a CI-pass commit to `branch`.
///
/// The committed workflow gates on `GITHUB_SHA`'s commit **message** containing
/// [`provision::CI_PASS_MARKER`] (a host-mode runner has no `actions/checkout`
/// offline, so a working-directory file check is unavailable — the gate reads the
/// commit through Forgejo's API instead). This commits a trivial file with
/// a marker-bearing message, producing a **new head SHA** whose CI run passes —
/// the second of the two verdicts `ci_fails_then_passes` asserts (a CI run is
/// keyed by SHA, findings-phase-0b). It is also used at PR-open time for the
/// non-CI-fail scenarios, so their head's latest commit carries the marker and
/// passes CI immediately.
///
/// `token` is any token authorized to write the repo (the engineer's own role
/// token). **Idempotent**: once the marker commit exists, a re-attempt re-commits
/// the same path and is tolerated as a `422` no-op (the new-SHA-producing,
/// marker-bearing commit already happened on the first success), so the worker
/// may re-run it every tick without erroring.
pub async fn commit_ci_sentinel(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    branch: &str,
) -> Result<()> {
    if branch.is_empty() {
        return Err(ProvisionError::Shape {
            what: "ci-sentinel branch".into(),
            detail: "target branch is empty".into(),
        });
    }
    let client = rest::http_client()?;
    let safe: String = branch
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    let path = format!("{CI_SENTINEL_DIR}/{safe}.txt");
    // The message MUST contain the marker; the gate reads exactly this.
    let message = format!("ci pass for {branch} {}", provision::CI_PASS_MARKER);
    rest::commit_file(
        &client,
        base_url,
        token,
        owner,
        name,
        &path,
        &format!("ci pass marker for {branch}\n"),
        &message,
        branch,
    )
    .await
}

/// The repo-relative path of the trivial prep file for `head`. Slashes in the
/// branch name are flattened so the file sits directly under [`PREP_DIR`].
fn prep_file_path(head: &str) -> String {
    let safe: String = head
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    format!("{PREP_DIR}/{safe}.txt")
}

/// A trivial, head-specific file body. The content only needs to make the head
/// diff from base; embedding the head name keeps distinct heads distinct.
fn prep_file_contents(head: &str) -> String {
    format!("PR head branch: {head}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prep_file_path_flattens_slashes_under_prep_dir() {
        assert_eq!(
            prep_file_path("fake/pr-for-code-3"),
            ".temper-pr-prep/fake-pr-for-code-3.txt"
        );
    }

    #[test]
    fn distinct_heads_get_distinct_paths_and_contents() {
        assert_ne!(
            prep_file_path("fake/pr-for-code-1"),
            prep_file_path("fake/pr-for-code-2")
        );
        assert_ne!(
            prep_file_contents("fake/pr-for-code-1"),
            prep_file_contents("fake/pr-for-code-2")
        );
    }
}
