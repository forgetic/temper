// SPDX-License-Identifier: MPL-2.0
//! Password/web-UI CI read path for the Forgejo backend (ADR 0019).
//!
//! Forgejo 7.0.x does not serve Actions runs/tasks over REST, so CI status is
//! read through the **password-authenticated web UI**, mirroring the production
//! pi tool (`forgejo-tools.ts`): a version-dependent login establishes a cookie
//! jar (with form/header CSRF on Forgejo 7), the `/{owner}/{repo}/actions` page
//! is scraped for run ids, and each run's status is read from the **live-view
//! JSON** (`POST .../runs/{run}/jobs/{job}/attempt/1` on Forgejo 15, with a
//! Forgejo 7 fallback) using the cookie jar, an optional `X-Csrf-Token` header,
//! and a `{"logCursors":[]}` body. The session re-logs in on a bounce back to
//! `/user/login` or a `401`/`403`.
//!
//! This module and its [`crate::ci_ui_parse`] sibling are the **only** code that
//! knows the web-UI HTML/JSON shapes. They are version-sensitive and best-effort
//! (ADR 0019): missing fields tolerate to `Queued`; list reads continue past
//! per-run HTTP failures while refusing older evidence behind an unreadable
//! boundary, and infrastructure-wide failures surface a portable [`ForgeError`].
//! Requests bypass [`crate::client::build_request`] (no `/api/v1` prefix, cookie
//! auth instead of the token, form-encoded bodies), so they are issued through
//! the raw [`HttpClient`] seam directly.
//!
//! The live-view JSON DTOs live in [`dto`], the authenticated session
//! (cookie jar, login, redirect/bounce handling, run discovery) in [`session`],
//! and the run → portable [`CiJob`] mapping in [`map`]. This module orchestrates
//! a read and is the only entry point the REST fallback in [`crate::ci`] calls.

mod dto;
mod map;
mod session;

use crate::ci_match::Target;
use crate::config::WebUiCredentials;
use crate::ids::{CiJobCoord, RepoCoord};
use crate::{ForgejoForge, HttpClient};
use map::live_run_to_jobs;
use session::{LiveViewOutcome, LiveViewUnreadable, WebUiClient};
use temper_forge_model::{CiJob, ForgeResult, RepositoryId};

/// Most-recent runs scraped per read. The Actions page lists runs newest-first,
/// and a CI read only ever cares about the target's current head — the latest
/// run on its branch, plus (for the fail→pass case) the immediately preceding
/// run on the same branch. Older runs belong to long-settled commits and pushing
/// a new commit replaces, not appends to, the relevant runs. Without this bound
/// the per-read cost grows with the repo's entire CI history (the idle-tick
/// storm: live-view POSTs for runs 29, 30, 31, …). Generous enough to cover
/// several concurrent open PRs' newest runs.
const MAX_RUNS_SCRAPED: usize = 20;

/// Reads CI jobs for a repository through the web UI, matching to `target`.
///
/// Logs in, discovers run ids from the Actions page, reads each run's live-view
/// JSON, keeps runs with provider SHA evidence for an explicit commit (or PR
/// branch/history matches for PR-only reads), and maps each job to a portable
/// [`CiJob`]. The run id doubles as the job-page index and the encoded run
/// coordinate. The returned jobs are unsorted/unfiltered; the caller applies
/// the query's status filter and sort, exactly as the REST path does.
pub(crate) async fn read_ci_jobs<C: HttpClient>(
    forge: &ForgejoForge<C>,
    credentials: &WebUiCredentials,
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    target: &Target,
) -> ForgeResult<Vec<CiJob>> {
    let mut client = WebUiClient::new(forge, credentials);
    client.login().await?;
    let mut run_ids = client.discover_run_ids(repo).await?;

    // Bound the scrape to the most-recent runs (newest-first page order). This
    // caps the per-read cost at a constant instead of growing with CI history,
    // while still covering the target's latest run and its fail→pass predecessor.
    if run_ids.len() > MAX_RUNS_SCRAPED {
        run_ids.truncate(MAX_RUNS_SCRAPED);
    }

    let mut jobs = Vec::new();
    let mut first_unreadable = None;
    let mut unreadable_count = 0usize;
    for run in run_ids {
        let live = match client.run_live_view(repo, run, 0).await? {
            LiveViewOutcome::Found(live) => live,
            LiveViewOutcome::Missing => continue,
            LiveViewOutcome::Unreadable(unreadable) => {
                unreadable_count += 1;
                first_unreadable.get_or_insert(unreadable);
                continue;
            }
        };
        // A caller-supplied commit is mandatory: only provider SHA evidence can
        // own that query. A different PR pseudo-ref is conclusive even when the
        // commit is shared, while runs without a PR pseudo-ref remain eligible
        // so push-based PR CI is preserved. PR-only reads retain same-branch
        // history so fail→pass diagnostics still expose both heads.
        let sha_ok = crate::ci_ui_parse::commit_matches(&live.commit.short_sha, target);
        let matches = if target.explicit_commit().is_some() {
            sha_ok && !branch_conflicts_with_pr(&live.commit.branch.name, target)
        } else {
            sha_ok || branch_matches(&live.commit.branch.name, target)
        };
        if !matches {
            continue;
        }
        // Runs are discovered newest-first. Once one cannot be read, an older
        // matching run may have been superseded by that unknown run and cannot
        // establish a verdict. Keep evidence already established on the newer
        // side of the boundary, but only inspect (never collect) older matches.
        if first_unreadable.is_some() {
            continue;
        }
        jobs.extend(live_run_to_jobs(repo, repo_id, run, &live, target));
    }
    if let Some(representative) = first_unreadable.as_ref() {
        warn_degraded_list_read(
            representative,
            unreadable_count,
            if jobs.is_empty() {
                "pending"
            } else {
                "continued"
            },
        );
    }
    Ok(jobs)
}

/// Emits one bounded, secret-free summary for a list read containing one or
/// more unreadable runs. Only the newest unreadable run is represented;
/// `omitted_count` tells operators how many additional per-run diagnostics were
/// folded into this event.
fn warn_degraded_list_read(
    representative: &LiveViewUnreadable,
    unreadable_count: usize,
    outcome: &'static str,
) {
    let repository = representative.repository.path_segment();
    let omitted_count = unreadable_count.saturating_sub(1);
    tracing::warn!(
        target: "temper_forge_forgejo",
        repository = %repository,
        run = representative.run,
        job = representative.job,
        status = u64::from(representative.final_http_status),
        retry_count = u64::from(representative.retry_count),
        unreadable_count,
        omitted_count,
        outcome,
        "forgejo web-ui degraded CI list read: repository {repository}, representative run {}, \
         job {}, status {}, retry count {}, unreadable count {unreadable_count}, omitted count \
         {omitted_count}, outcome {outcome}",
        representative.run,
        representative.job,
        representative.final_http_status,
        representative.retry_count,
    );
}

/// Whether a run explicitly identifies a different pull request.
///
/// Source branch names and blank branch values carry no PR identity and do not
/// conflict. Forgejo's `#<number>` pseudo-ref does, so it can safely prevent a
/// shared commit's workflow from leaking between pull requests.
fn branch_conflicts_with_pr(branch: &str, target: &Target) -> bool {
    let run_pr = branch
        .strip_prefix('#')
        .and_then(|number| number.parse::<u64>().ok());
    matches!((target.pr_number, run_pr), (Some(expected), Some(actual)) if expected != actual)
}

/// Whether a run's commit branch identifies the target pull request. Forgejo's
/// live view has used both the source branch (`agent/pr-for-code-1`) and the PR
/// pseudo-ref (`#2`) for pull-request runs, so accept either form. A run with no
/// branch still falls back to the SHA axis.
fn branch_matches(branch: &str, target: &Target) -> bool {
    if branch.is_empty() {
        return false;
    }
    if let Some(number) = target.pr_number {
        if branch == format!("#{number}") {
            return true;
        }
    }
    match target.pr_head_ref.as_deref() {
        Some(head_ref) if !head_ref.is_empty() => branch == head_ref,
        _ => false,
    }
}

/// Reads a single CI job by its decoded coordinate through the web UI.
///
/// Logs in, reads the run's live-view JSON, and returns the job at the encoded
/// `job_index` (or `None` when the run/job is absent). Used as the `get_ci_job`
/// fallback when the REST Actions endpoint is unavailable (ADR 0019).
pub(crate) async fn read_ci_job<C: HttpClient>(
    forge: &ForgejoForge<C>,
    credentials: &WebUiCredentials,
    coord: &CiJobCoord,
    repo_id: &RepositoryId,
) -> ForgeResult<Option<CiJob>> {
    let mut client = WebUiClient::new(forge, credentials);
    client.login().await?;
    let live = match client.run_live_view(&coord.repo, coord.run, 0).await? {
        LiveViewOutcome::Found(live) => live,
        LiveViewOutcome::Missing => return Ok(None),
        LiveViewOutcome::Unreadable(unreadable) => {
            return Err(unreadable.into_backend_error());
        }
    };
    let target = Target::default();
    let jobs = live_run_to_jobs(&coord.repo, repo_id, coord.run, &live, &target);
    Ok(jobs.into_iter().nth(coord.job_index as usize))
}
