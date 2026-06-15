// SPDX-License-Identifier: MPL-2.0
//! Password/web-UI CI read path for the Forgejo backend (ADR 0019).
//!
//! Forgejo 7.0.x does not serve Actions runs/tasks over REST, so CI status is
//! read through the **password-authenticated web UI**, mirroring the production
//! pi tool (`forgejo-tools.ts`): a CSRF login establishes a cookie jar, the
//! `/{owner}/{repo}/actions` page is scraped for run ids, and each run's status
//! is read from the **live-view JSON** (`POST .../runs/{run}/jobs/{job}` with an
//! `X-Csrf-Token` header and a `{"logCursors":[]}` body). The session re-logs in
//! on a bounce back to `/user/login` or a `401`/`403`.
//!
//! This module and its [`crate::ci_ui_parse`] sibling are the **only** code that
//! knows the web-UI HTML/JSON shapes. They are version-sensitive and best-effort
//! (ADR 0019): missing fields tolerate to `Queued`, and any hard failure
//! surfaces a portable [`ForgeError`] rather than guessing a pass/fail verdict.
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
use session::WebUiClient;
use temper_forge::{CiJob, ForgeResult, RepositoryId};

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
/// JSON, keeps runs whose commit short-SHA matches the target (or every run when
/// the target carries no filter), and maps each job to a portable [`CiJob`]. The
/// run id doubles as the job-page index and the encoded run coordinate. The
/// returned jobs are unsorted/unfiltered; the caller applies the query's status
/// filter and sort, exactly as the REST path does.
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
    for run in run_ids {
        let Some(live) = client.run_live_view(repo, run, 0).await? else {
            continue;
        };
        // Keep a run when its commit matches the target head SHA **or** its commit
        // sits on the target's head branch. The branch match is essential for the
        // fail→pass case: the first (failing) run and the fixed (passing) run live
        // on **different** SHAs of the same head branch, so a SHA-only filter would
        // drop the failing verdict and the engine would never see "fail then pass".
        let sha_ok = crate::ci_ui_parse::commit_matches(&live.commit.short_sha, target);
        let branch_ok = branch_matches(&live.commit.branch.name, target);
        if !(sha_ok || branch_ok) {
            continue;
        }
        // Drop superseded (cancelled) runs: when several commits are pushed to a
        // head in quick succession Forgejo cancels the in-flight runs, but a
        // cancelled run carries no verdict — it is neither a pass nor a fail. The
        // reference CI producer emits only real verdicts, so excluding cancelled
        // runs keeps the Forgejo verdict stream the same shape (e.g. a clean
        // `[Failure, Success]` for fail→pass) and stops a stray cancellation from
        // masking the real latest verdict in `CiStatus::from_jobs`.
        for job in live_run_to_jobs(repo, repo_id, run, &live, target) {
            if job.conclusion == Some(temper_forge::CiJobConclusion::Cancelled) {
                continue;
            }
            jobs.push(job);
        }
    }
    Ok(jobs)
}

/// Whether a run's commit branch matches the target's head ref. A run with no
/// branch, or a target with no head ref, does not match on this axis (the SHA
/// axis still applies).
fn branch_matches(branch: &str, target: &Target) -> bool {
    match target.pr_head_ref.as_deref() {
        Some(head_ref) if !head_ref.is_empty() && !branch.is_empty() => branch == head_ref,
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
    let Some(live) = client.run_live_view(&coord.repo, coord.run, 0).await? else {
        return Ok(None);
    };
    let target = Target::default();
    let jobs = live_run_to_jobs(&coord.repo, repo_id, coord.run, &live, &target);
    Ok(jobs.into_iter().nth(coord.job_index as usize))
}
