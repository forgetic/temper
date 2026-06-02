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

use crate::ci::map_status;
use crate::ci_match::Target;
use crate::ci_ui_parse::{
    commit_matches, cookie_header, extract_input_value, extract_run_ids, first_non_empty,
    form_encode, is_login_bounce, is_login_redirect, login_succeeded, redirect_location,
    store_cookies,
};
use crate::config::WebUiCredentials;
use crate::ids::{format_ci_job_id, CiJobCoord, RepoCoord};
use crate::{ForgejoForge, HttpClient, HttpMethod, HttpRequest, HttpResponse};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use temper_forge::{CiJob, CiJobStatus, ForgeError, ForgeResult, PullRequestId, RepositoryId};

/// Maximum redirects followed for a single web-UI request.
const MAX_REDIRECTS: usize = 8;

/// Cookies the Forgejo session login establishes, in name-sorted order.
#[derive(Clone, Debug, Default)]
struct CookieJar {
    cookies: BTreeMap<String, String>,
}

impl CookieJar {
    /// Records any `Set-Cookie` headers from a response into the jar.
    fn store(&mut self, response: &HttpResponse) {
        store_cookies(&mut self.cookies, response);
    }

    /// Returns the `Cookie` request header value, or `None` when empty.
    fn header(&self) -> Option<String> {
        cookie_header(&self.cookies)
    }

    /// Returns the value of the `_csrf` cookie, used as the `X-Csrf-Token`.
    fn csrf(&self) -> Option<&str> {
        self.cookies.get("_csrf").map(String::as_str)
    }
}

/// Live-view JSON returned by `POST …/runs/{run}/jobs/{job}`.
#[derive(Debug, Default, Deserialize)]
struct LiveViewDto {
    #[serde(default)]
    state: LiveStateDto,
}

#[derive(Debug, Default, Deserialize)]
struct LiveStateDto {
    #[serde(default)]
    run: LiveRunDto,
}

#[derive(Debug, Default, Deserialize)]
struct LiveRunDto {
    // Decoded for provider-shape fidelity; the portable verdict is derived from
    // the per-job statuses, so the run-level status is not read directly.
    #[serde(default)]
    #[allow(dead_code)]
    status: String,
    #[serde(default)]
    jobs: Vec<LiveJobDto>,
    #[serde(default)]
    commit: LiveCommitDto,
}

#[derive(Debug, Default, Deserialize)]
struct LiveJobDto {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Default, Deserialize)]
struct LiveCommitDto {
    #[serde(default, rename = "shortSHA")]
    short_sha: String,
    #[serde(default)]
    branch: LiveBranchDto,
}

#[derive(Debug, Default, Deserialize)]
struct LiveBranchDto {
    #[serde(default)]
    name: String,
}

/// Web-UI session bound to a [`ForgejoForge`] backend and its credentials.
pub(crate) struct WebUiClient<'a, C: HttpClient> {
    forge: &'a ForgejoForge<C>,
    credentials: &'a WebUiCredentials,
    jar: CookieJar,
}

impl<'a, C: HttpClient> WebUiClient<'a, C> {
    /// Builds an unauthenticated session; call [`Self::login`] before fetching.
    pub(crate) fn new(forge: &'a ForgejoForge<C>, credentials: &'a WebUiCredentials) -> Self {
        Self {
            forge,
            credentials,
            jar: CookieJar::default(),
        }
    }

    /// Issues one raw web-UI request through the HTTP seam (no API prefix/token).
    async fn execute(&mut self, request: HttpRequest) -> ForgeResult<HttpResponse> {
        let response = self
            .forge
            .http_client()
            .execute(request)
            .await
            .map_err(crate::error::map_transport_error)?;
        self.jar.store(&response);
        Ok(response)
    }

    /// Performs the CSRF login handshake, populating the cookie jar.
    ///
    /// GET `/user/login` to capture the CSRF token and initial cookies, then POST
    /// the form-encoded credentials. A `200`, a redirect back to `/user/login`,
    /// or a non-redirect error is treated as a failed login (the password is
    /// never echoed into the error).
    pub(crate) async fn login(&mut self) -> ForgeResult<()> {
        self.jar = CookieJar::default();
        let page = self
            .execute(self.request(HttpMethod::Get, "/user/login", Vec::new(), None))
            .await?;
        let csrf = extract_input_value(&page.body, "_csrf");

        let mut form = vec![
            ("user_name", self.credentials.username.as_str()),
            ("password", self.credentials.password.as_str()),
            ("remember", "on"),
        ];
        if let Some(csrf) = csrf.as_deref() {
            form.push(("_csrf", csrf));
        }
        let body = form_encode(&form);
        let headers = vec![(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        let response = self
            .execute(self.request(HttpMethod::Post, "/user/login", headers, Some(body)))
            .await?;

        if login_succeeded(&response) {
            Ok(())
        } else {
            Err(ForgeError::Backend(format!(
                "forgejo web-ui login failed (status {})",
                response.status
            )))
        }
    }

    /// Fetches a web-UI path with the cookie jar, following redirects and
    /// re-logging in once on a bounce to the login page.
    async fn fetch(
        &mut self,
        method: HttpMethod,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let response = self
            .fetch_following(method, path, headers.clone(), body.clone())
            .await?;
        if is_login_bounce(&response) {
            self.login().await?;
            return self.fetch_following(method, path, headers, body).await;
        }
        Ok(response)
    }

    /// Fetches a path, following up to [`MAX_REDIRECTS`] redirects.
    ///
    /// A redirect to the login page is a session bounce, not a normal redirect:
    /// it is returned to [`Self::fetch`] so the caller can re-login and retry.
    async fn fetch_following(
        &mut self,
        method: HttpMethod,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let mut current = path.to_string();
        for _ in 0..MAX_REDIRECTS {
            let request = self.request(method, &current, headers.clone(), body.clone());
            let response = self.execute(request).await?;
            match redirect_location(&response) {
                Some(location) if is_login_redirect(&location) => return Ok(response),
                Some(location) => current = location,
                None => return Ok(response),
            }
        }
        Err(ForgeError::Backend(format!(
            "forgejo web-ui: too many redirects fetching {path}"
        )))
    }

    /// Builds a raw web-UI request: a host-relative `path` (no `/api/v1`), the
    /// cookie jar header, and `Accept: text/html` unless overridden.
    fn request(
        &self,
        method: HttpMethod,
        path: &str,
        mut headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> HttpRequest {
        if let Some(cookie) = self.jar.header() {
            headers.push(("Cookie".to_string(), cookie));
        }
        if !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
        {
            headers.push((
                "Accept".to_string(),
                "text/html,application/xhtml+xml".to_string(),
            ));
        }
        HttpRequest {
            method,
            path: path.to_string(),
            query: Vec::new(),
            headers,
            body,
        }
    }

    /// Scrapes the run ids visible on the repository Actions page.
    async fn discover_run_ids(&mut self, repo: &RepoCoord) -> ForgeResult<Vec<u64>> {
        let path = format!("/{}/{}/actions", repo.owner, repo.name);
        let response = self.fetch(HttpMethod::Get, &path, Vec::new(), None).await?;
        if !response.is_success() {
            return Err(ForgeError::Backend(format!(
                "forgejo web-ui: actions page for {} returned status {}",
                repo.path_segment(),
                response.status
            )));
        }
        Ok(extract_run_ids(&response.body))
    }

    /// Reads one run's live-view JSON (`POST …/runs/{run}/jobs/{job}`).
    ///
    /// Returns `None` when the run/job page is absent (`404`); other non-success
    /// statuses are hard errors so a missing verdict never reads as a pass/fail.
    async fn run_live_view(
        &mut self,
        repo: &RepoCoord,
        run: u64,
        job: u64,
    ) -> ForgeResult<Option<LiveRunDto>> {
        let path = format!(
            "/{}/{}/actions/runs/{run}/jobs/{job}",
            repo.owner, repo.name
        );
        let mut headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        if let Some(csrf) = self.jar.csrf() {
            headers.push(("X-Csrf-Token".to_string(), csrf.to_string()));
        }
        let response = self
            .fetch(
                HttpMethod::Post,
                &path,
                headers,
                Some("{\"logCursors\":[]}".to_string()),
            )
            .await?;
        if response.status == 404 {
            return Ok(None);
        }
        if !response.is_success() {
            return Err(ForgeError::Backend(format!(
                "forgejo web-ui: live view for run {run} returned status {}",
                response.status
            )));
        }
        let dto: LiveViewDto = serde_json::from_str(&response.body).map_err(|error| {
            ForgeError::Backend(format!(
                "forgejo web-ui: failed to decode live view: {error}"
            ))
        })?;
        Ok(Some(dto.state.run))
    }
}

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
    let run_ids = client.discover_run_ids(repo).await?;

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
        let sha_ok = commit_matches(&live.commit.short_sha, target);
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

/// Maps a run's live-view jobs to portable [`CiJob`]s.
fn live_run_to_jobs(
    repo: &RepoCoord,
    repo_id: &RepositoryId,
    run: u64,
    live: &LiveRunDto,
    target: &Target,
) -> Vec<CiJob> {
    let commit_sha = first_non_empty(&[
        live.commit.short_sha.as_str(),
        target.pr_head_sha.as_deref().unwrap_or_default(),
        target.commit_sha.as_deref().unwrap_or_default(),
    ]);
    let pull_request_id: Option<PullRequestId> = target.pr_id.clone();

    live.jobs
        .iter()
        .enumerate()
        .map(|(index, job)| {
            let (status, conclusion) = map_status(&job.status);
            let name = if job.name.is_empty() {
                format!("job-{index}")
            } else {
                job.name.clone()
            };
            let coord = CiJobCoord {
                repo: repo.clone(),
                run,
                job_index: index as u64,
                // The web UI exposes no stable task id; the run id is the page's
                // job coordinate, so reuse it as the encoded task id.
                task_id: run,
            };
            // The live view exposes no per-job timestamp, but CI runs are created
            // in execution order, so the run id is monotonic in time. Derive
            // `created_at`/`updated_at` from it (epoch + run seconds) so the
            // portable ordering by `created_at` reflects "older run before newer".
            // This is what lets `CiStatus::from_jobs` pick the latest run per job
            // (the merge gate) and the `ci_fails_then_passes` assert read the
            // failing run before the fixed, passing one — both keyed on real
            // ordering rather than a shared epoch that would tie every run.
            let run_time = run_ordering_time(run);
            let completed_at = (status == CiJobStatus::Completed).then_some(run_time);
            CiJob {
                id: format_ci_job_id(&coord),
                repo_id: repo_id.clone(),
                pull_request_id: pull_request_id.clone(),
                commit_sha: commit_sha.clone(),
                name,
                status,
                conclusion,
                url: None,
                created_at: run_time,
                started_at: None,
                completed_at,
                updated_at: run_time,
            }
        })
        .collect()
}

/// A monotonic ordering timestamp derived from a run id.
///
/// The web UI exposes no per-job timestamp; run ids increase with creation time,
/// so `epoch + run_id` seconds gives a stable, run-ordered timestamp. It is an
/// **ordering** key, not a wall-clock truth — only the relative order of runs is
/// meaningful (newer run ⇒ later timestamp).
fn run_ordering_time(run: u64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(run as i64, 0).unwrap_or_else(epoch)
}

/// The unix epoch fallback.
fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch is a valid timestamp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::CiJobConclusion;

    #[test]
    fn cookie_jar_stores_and_exposes_csrf() {
        let mut jar = CookieJar::default();
        let mut response = HttpResponse::new(200, "");
        response.headers = vec![
            (
                "Set-Cookie".to_string(),
                "i_like_gitea=abc; Path=/".to_string(),
            ),
            ("set-cookie".to_string(), "_csrf=tok; Path=/".to_string()),
        ];
        jar.store(&response);
        let header = jar.header().unwrap();
        assert!(header.contains("i_like_gitea=abc"));
        assert!(header.contains("_csrf=tok"));
        assert_eq!(jar.csrf(), Some("tok"));
    }

    #[test]
    fn live_run_maps_jobs_to_portable_status() {
        let repo = RepoCoord::new("acme", "widgets");
        let repo_id = RepositoryId::new("forgejo:acme/widgets");
        let live = LiveRunDto {
            status: "failure".to_string(),
            jobs: vec![
                LiveJobDto {
                    name: "build".to_string(),
                    status: "failure".to_string(),
                },
                LiveJobDto {
                    name: String::new(),
                    status: "running".to_string(),
                },
            ],
            commit: LiveCommitDto {
                short_sha: "c456eec18b".to_string(),
                branch: LiveBranchDto {
                    name: "main".to_string(),
                },
            },
        };
        let target = Target {
            pr_id: Some(PullRequestId::new("forgejo:acme/widgets:pull:7")),
            ..Default::default()
        };
        let jobs = live_run_to_jobs(&repo, &repo_id, 1, &live, &target);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "build");
        assert_eq!(jobs[0].status, CiJobStatus::Completed);
        assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Failure));
        assert_eq!(jobs[0].id.as_str(), "forgejo:acme/widgets:actions:1:0:1");
        assert_eq!(jobs[0].commit_sha, "c456eec18b");
        assert_eq!(
            jobs[0].pull_request_id.as_ref().unwrap().as_str(),
            "forgejo:acme/widgets:pull:7"
        );
        // Fallback job name and running status.
        assert_eq!(jobs[1].name, "job-1");
        assert_eq!(jobs[1].status, CiJobStatus::Running);
        assert_eq!(jobs[1].conclusion, None);
    }

    #[test]
    fn live_view_decodes_nested_state() {
        let dto: LiveViewDto = serde_json::from_str(
            r#"{"state":{"run":{"status":"success","jobs":[{"name":"build","status":"success"}],
               "commit":{"shortSHA":"abc1234"}}},"logs":{}}"#,
        )
        .unwrap();
        assert_eq!(dto.state.run.jobs.len(), 1);
        assert_eq!(dto.state.run.commit.short_sha, "abc1234");
    }
}
