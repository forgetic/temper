//! REST Actions fetching for the CI read path: workflow runs, strict per-run
//! jobs, and the pull-request head lookup used for run matching.

use crate::ids::RepoCoord;
use crate::map::pr_branch_name;
use crate::types::{ActionJobDto, ActionRunDto, PullRequestDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use temper_forge_model::{ForgeError, ForgeResult, ItemNumber};

/// Fixed provider page size for Actions run inventory.
const ACTIONS_PAGE_LIMIT: usize = 50;
/// Maximum Actions run-list requests made by one logical CI read.
const ACTIONS_MAX_PAGES: u32 = 64;
const ACTIONS_ENDPOINT: &str = "/api/v1/repos/{owner}/{repo}/actions/runs";
const ACTIONS_OPERATION: &str = "list_runs";
const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

#[derive(Copy, Clone)]
enum ActionsPaginationFailure {
    Transport,
    Status(u16),
    Malformed,
    OversizedPage,
    NonAdvancingPage,
    PageCeiling,
}

impl ActionsPaginationFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Status(_) => "status",
            Self::Malformed => "malformed",
            Self::OversizedPage => "oversized_page",
            Self::NonAdvancingPage => "non_advancing_page",
            Self::PageCeiling => "page_ceiling",
        }
    }

    const fn status(self) -> Option<u16> {
        match self {
            Self::Status(status) => Some(status),
            _ => None,
        }
    }
}

struct RunsPageDecodeError {
    rows: Option<usize>,
}

fn bounded_fact(value: Option<usize>, maximum: usize) -> String {
    match value {
        None => "unknown".to_string(),
        Some(value) if value <= maximum => value.to_string(),
        Some(_) => format!(">{maximum}"),
    }
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Fetches a pull request's head SHA and head ref for run matching.
    ///
    /// Returns `None` when the pull request is absent (`404`). Reuses the
    /// existing [`PullRequestDto`] rather than introducing a CI-only DTO.
    pub(super) async fn fetch_pr_head(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<(Option<String>, Option<String>)>> {
        let path = format!("/repos/{}/pulls/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional(
                "get pull request for CI",
                HttpMethod::Get,
                &path,
                Vec::new(),
                None,
            )
            .await?
        else {
            return Ok(None);
        };
        let dto: PullRequestDto = Self::decode("get pull request for CI", &response)?;
        let head = dto.head.unwrap_or_default();
        let branch = pr_branch_name(&head);
        Ok(Some((head.sha.filter(|sha| !sha.is_empty()), branch)))
    }

    /// Fetches and aggregates a bounded sequence of Actions run pages.
    ///
    /// Traversal starts at page one, and every request carries both `page` and
    /// `limit`.
    /// A short (including empty) page terminates the traversal. Sixty-four full
    /// pages, an oversized page, or repeated provider run identities fail
    /// closed; no unpaged or non-API fallback is attempted.
    pub(super) async fn fetch_actions_runs(&self, path: &str) -> ForgeResult<Vec<ActionRunDto>> {
        let mut runs = Vec::new();
        let mut seen_run_ids = HashSet::new();

        for page in 1..=ACTIONS_MAX_PAGES {
            // This path deliberately does not use a generic list helper or an
            // unpaged retry. Every physical request is fully page-qualified.
            let query = vec![
                ("page".to_string(), page.to_string()),
                ("limit".to_string(), ACTIONS_PAGE_LIMIT.to_string()),
            ];
            let response = match self.send(HttpMethod::Get, path, query, None).await {
                Ok(response) => response,
                Err(_) => {
                    return Err(actions_pagination_error(
                        page,
                        ActionsPaginationFailure::Transport,
                        None,
                        None,
                    ));
                }
            };
            let response_bytes = response.body.len();
            if !response.is_success() {
                return Err(actions_pagination_error(
                    page,
                    ActionsPaginationFailure::Status(response.status),
                    Some(response_bytes),
                    None,
                ));
            }
            let page_runs = extract_runs_page(&response.body).map_err(|error| {
                actions_pagination_error(
                    page,
                    ActionsPaginationFailure::Malformed,
                    Some(response_bytes),
                    error.rows,
                )
            })?;
            let page_len = page_runs.len();
            if page_len > ACTIONS_PAGE_LIMIT {
                return Err(actions_pagination_error(
                    page,
                    ActionsPaginationFailure::OversizedPage,
                    Some(response_bytes),
                    Some(page_len),
                ));
            }
            if page_runs.iter().any(|run| run.id == 0)
                || page_runs.iter().any(|run| !seen_run_ids.insert(run.id))
            {
                return Err(actions_pagination_error(
                    page,
                    ActionsPaginationFailure::NonAdvancingPage,
                    Some(response_bytes),
                    Some(page_len),
                ));
            }
            runs.extend(page_runs);
            if page_len < ACTIONS_PAGE_LIMIT {
                return Ok(runs);
            }
            if page == ACTIONS_MAX_PAGES {
                return Err(actions_pagination_error(
                    page,
                    ActionsPaginationFailure::PageCeiling,
                    Some(response_bytes),
                    Some(page_len),
                ));
            }
        }

        unreachable!("bounded Actions page loop always returns")
    }

    /// Fetches and validates one provider run's Forgejo 16 jobs response.
    ///
    /// The endpoint coordinate is the run database `id`. Every returned row
    /// must carry a non-zero provider run/job identity and must identify that
    /// same run. Provider-reported zero attempt/task values are retained because
    /// Forgejo uses them before a queued job is assigned to a runner. Forgejo's
    /// bare JSON array is required; an explicit `[]` is a successful empty list.
    pub(super) async fn fetch_run_jobs(
        &self,
        repo: &RepoCoord,
        run_id: u64,
    ) -> ForgeResult<Vec<ActionJobDto>> {
        let context = format!("list Forgejo Actions jobs for provider run {run_id}");
        if run_id == 0 {
            return Err(ForgeError::Backend(format!(
                "{context}: invalid zero provider run id"
            )));
        }
        let path = jobs_path(repo, run_id);
        let response = self.send(HttpMethod::Get, &path, Vec::new(), None).await?;
        if !response.is_success() {
            return Err(ForgeError::Backend(format!(
                "{context}: provider jobs unavailable (HTTP {})",
                response.status
            )));
        }
        let jobs: Vec<ActionJobDto> = serde_json::from_str(&response.body).map_err(|error| {
            ForgeError::Backend(format!(
                "{context}: failed to decode expected jobs array: {error}"
            ))
        })?;
        validate_jobs(&context, run_id, &jobs)?;
        Ok(current_attempt(jobs))
    }
}

pub(super) fn runs_path(repo: &RepoCoord) -> String {
    format!("/repos/{}/actions/runs", repo.path_segment())
}

pub(super) fn jobs_path(repo: &RepoCoord, run_id: u64) -> String {
    format!("/repos/{}/actions/runs/{run_id}/jobs", repo.path_segment())
}

/// Decodes one of Forgejo's two established wrapped run-list shapes.
fn extract_runs_page(body: &str) -> Result<Vec<ActionRunDto>, RunsPageDecodeError> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| RunsPageDecodeError { rows: None })?;
    for key in ["workflow_runs", "runs"] {
        match value.get(key) {
            None | Some(Value::Null) => continue,
            Some(Value::Array(rows)) => {
                let row_count = rows.len();
                return serde_json::from_value(Value::Array(rows.clone())).map_err(|_| {
                    RunsPageDecodeError {
                        rows: Some(row_count),
                    }
                });
            }
            Some(_) => return Err(RunsPageDecodeError { rows: None }),
        }
    }
    Err(RunsPageDecodeError { rows: None })
}

fn actions_pagination_error(
    page: u32,
    failure: ActionsPaginationFailure,
    response_bytes: Option<usize>,
    response_rows: Option<usize>,
) -> ForgeError {
    let status = failure
        .status()
        .map_or_else(|| "none".to_string(), |status| status.to_string());
    let response_bytes = bounded_fact(response_bytes, MAX_DIAGNOSTIC_BYTES);
    let response_rows = bounded_fact(response_rows, ACTIONS_PAGE_LIMIT + 1);
    tracing::warn!(
        target: "temper_forge_forgejo",
        endpoint = ACTIONS_ENDPOINT,
        operation = ACTIONS_OPERATION,
        page,
        limit = ACTIONS_PAGE_LIMIT,
        status = %status,
        failure_class = failure.code(),
        response_bytes = %response_bytes,
        response_rows = %response_rows,
        "Forgejo Actions pagination failed"
    );
    ForgeError::Backend(format!(
        "Forgejo Actions pagination failed: endpoint={ACTIONS_ENDPOINT} \
         operation={ACTIONS_OPERATION} page={page} limit={ACTIONS_PAGE_LIMIT} \
         status={status} failure={} response_bytes={response_bytes} \
         response_rows={response_rows}",
        failure.code()
    ))
}

fn validate_jobs(context: &str, run_id: u64, jobs: &[ActionJobDto]) -> ForgeResult<()> {
    let mut identities = HashSet::new();
    for job in jobs {
        if job.id == 0 || job.run_id == 0 {
            return Err(ForgeError::Backend(format!(
                "{context}: invalid zero provider run or job identity"
            )));
        }
        if job.run_id != run_id {
            return Err(ForgeError::Backend(format!(
                "{context}: job {} belongs to provider run {}, expected {run_id}",
                job.id, job.run_id
            )));
        }
        if job.name.trim().is_empty() || job.status.trim().is_empty() {
            return Err(ForgeError::Backend(format!(
                "{context}: job {} has an empty required name or status",
                job.id
            )));
        }
        if !identities.insert((job.id, job.attempt)) {
            return Err(ForgeError::Backend(format!(
                "{context}: duplicate provider job attempt identity"
            )));
        }
    }
    Ok(())
}

/// Selects each stable provider job's largest provider-reported attempt.
///
/// Forgejo's endpoint normally returns one row per `ActionRunJob`, whose
/// `attempt` and `task_id` are already that job's latest values. Attempts may
/// legitimately differ between jobs, so a run-wide maximum must not discard
/// unaffected jobs. Grouping by provider job id also remains deterministic if
/// a provider ever returns more than one attempt row. Names and response order
/// never determine attempts.
fn current_attempt(jobs: Vec<ActionJobDto>) -> Vec<ActionJobDto> {
    let mut current = BTreeMap::new();
    for job in jobs {
        match current.entry(job.id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(job);
            }
            std::collections::btree_map::Entry::Occupied(mut entry)
                if job.attempt > entry.get().attempt =>
            {
                entry.insert(job);
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    current.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, attempt: u64, task_id: u64, name: &str) -> ActionJobDto {
        ActionJobDto {
            id,
            run_id: 900,
            attempt,
            task_id,
            name: name.to_string(),
            status: "success".to_string(),
        }
    }

    #[test]
    fn current_attempt_is_selected_per_provider_job() {
        let jobs = current_attempt(vec![
            job(33, 2, 44, "build"),
            job(11, 1, 41, "build"),
            job(11, 2, 43, "build"),
            job(22, 1, 42, "test"),
        ]);
        assert_eq!(
            jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
            [11, 22, 33]
        );
        assert_eq!(
            jobs.iter().map(|job| job.attempt).collect::<Vec<_>>(),
            [2, 1, 2]
        );
        assert_eq!(jobs[0].task_id, 43);
    }

    #[test]
    fn validates_run_scoped_identity_and_accepts_unassigned_jobs() {
        let valid = job(31, 1, 41, "build");
        assert!(validate_jobs("ctx", 900, std::slice::from_ref(&valid)).is_ok());

        let mut unassigned = valid.clone();
        unassigned.attempt = 0;
        unassigned.task_id = 0;
        assert!(validate_jobs("ctx", 900, &[unassigned]).is_ok());

        let mut mismatch = valid.clone();
        mismatch.run_id = 901;
        assert!(validate_jobs("ctx", 900, &[mismatch]).is_err());

        let mut invalid = valid;
        invalid.id = 0;
        assert!(validate_jobs("ctx", 900, &[invalid]).is_err());
    }
}
