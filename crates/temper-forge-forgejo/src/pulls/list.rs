//! Pull-request list paths for Forgejo.
//!
//! Labelled summary scans can use Forgejo's PR-as-issue rows directly, avoiding
//! per-PR detail reads on hot paths. Exact/detail behaviour stays in the parent
//! module's single-PR materialization helpers.

use super::*;
use crate::ids::format_pull_request_id;
use crate::map::map_issue;
use crate::types::IssueDto;
use std::cmp::Ordering;
use temper_forge_model::{
    BranchRef, ItemSortField, PullRequestQuery, PullRequestState, SortDirection,
};

impl<C: HttpClient> ForgejoForge<C> {
    /// Lists pull requests in a repository, filtered and sorted per `query`.
    ///
    /// Unlabelled queries use `/pulls` state filtering. Labelled full-detail
    /// queries keep the exact `/pulls/{number}` materialization. Labelled
    /// summary queries use the issue endpoint (`type=pulls`, `state`, `labels`)
    /// directly when the row contains enough state to answer the query; only
    /// ambiguous rows fall back to `/pulls/{number}`.
    pub async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        let mut pulls = if query.labels.is_empty() {
            self.list_pull_requests_by_state(&repo, &query).await?
        } else {
            self.list_labeled_pull_requests(&repo, &query).await?
        };
        pulls.sort_by(|left, right| compare_pull_requests(left, right, &query));
        if let Some(limit) = query.limit {
            pulls.truncate(limit);
        }
        if query.details.dependencies {
            for pull in &mut pulls {
                pull.dependencies = self.load_item_dependencies(&repo, pull.number).await?;
            }
        }
        Ok(pulls)
    }

    /// Lists pull requests through the `/pulls` endpoint for queries without a
    /// label filter. Forgejo can narrow these by state provider-side.
    async fn list_pull_requests_by_state(
        &self,
        repo: &RepoCoord,
        query: &PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let path = format!("/repos/{}/pulls", repo.path_segment());
        let base_query = vec![(
            "state".to_string(),
            pull_request_state_param(query.state).to_string(),
        )];
        let dtos: Vec<PullRequestDto> = self
            .list_up_to("list pull requests", &path, base_query, query.limit)
            .await?;
        let mut pulls = Vec::new();
        for dto in dtos {
            let pull = if dto.merged && dto.merged_by.is_none() {
                // Forgejo 15 omits `merged_by` from `/pulls` list rows even
                // though `/pulls/{number}` includes it. Re-fetch only those
                // merged summaries so automation/audit callers do not mistake
                // the PR author for the merger via the DTO mapper's last-ditch
                // fallback.
                self.fetch_pull_request(repo, ItemNumber::new(dto.number))
                    .await?
                    .unwrap_or_else(|| self.materialize_pull_request(repo, dto, None))
            } else {
                self.materialize_pull_request(repo, dto, None)
            };
            if pull_matches_query(&pull, query) {
                pulls.push(pull);
            }
        }
        Ok(pulls)
    }

    /// Lists labelled pull requests through Forgejo's issue label index.
    async fn list_labeled_pull_requests(
        &self,
        repo: &RepoCoord,
        query: &PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let rows = self.discover_labeled_pull_request_rows(repo, query).await?;
        if query.details.dependencies {
            self.materialize_labeled_pull_request_details(repo, rows, query)
                .await
        } else {
            self.materialize_labeled_pull_request_summaries(repo, rows, query)
                .await
        }
    }

    /// Uses Forgejo's issue endpoint as the provider-specific PR-label index.
    /// Asks for `type=pulls`, state, and labels; no `/pulls?state=all` fallback,
    /// so provider failures surface as diagnosable backend errors.
    async fn discover_labeled_pull_request_rows(
        &self,
        repo: &RepoCoord,
        query: &PullRequestQuery,
    ) -> ForgeResult<Vec<IssueDto>> {
        let path = format!("/repos/{}/issues", repo.path_segment());
        let base_query = vec![
            (
                "state".to_string(),
                pull_request_state_param(query.state).to_string(),
            ),
            ("type".to_string(), "pulls".to_string()),
            ("labels".to_string(), query.labels.join(",")),
        ];
        self.list_up_to(
            "discover labeled pull requests",
            &path,
            base_query,
            query.limit,
        )
        .await
    }

    /// Materializes labelled candidates exactly through `/pulls/{number}`.
    async fn materialize_labeled_pull_request_details(
        &self,
        repo: &RepoCoord,
        rows: Vec<IssueDto>,
        query: &PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let mut pulls = Vec::new();
        let mut seen = Vec::new();
        for row in rows {
            if !row.is_pull_request() {
                continue;
            }
            let number = ItemNumber::new(row.number);
            if seen.contains(&number) {
                continue;
            }
            seen.push(number);
            let Some(pull) = self.fetch_pull_request(repo, number).await? else {
                continue;
            };
            if pull_matches_query(&pull, query) {
                pulls.push(pull);
            }
        }
        Ok(pulls)
    }

    /// Materializes labelled summary candidates from the PR-as-issue rows when
    /// they are unambiguous; falls back to exact details for rows missing fields
    /// that the query needs (notably closed-vs-merged marker data).
    async fn materialize_labeled_pull_request_summaries(
        &self,
        repo: &RepoCoord,
        rows: Vec<IssueDto>,
        query: &PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let mut pulls = Vec::new();
        let mut seen = Vec::new();
        for row in rows {
            if !row.is_pull_request() {
                continue;
            }
            let number = ItemNumber::new(row.number);
            if seen.contains(&number) {
                continue;
            }
            seen.push(number);

            let pull = if pr_issue_row_can_answer_summary_query(&row, query) {
                Some(self.materialize_pull_request_summary(repo, row))
            } else {
                self.fetch_pull_request(repo, number).await?
            };
            if let Some(pull) = pull.filter(|pull| pull_matches_query(pull, query)) {
                pulls.push(pull);
            }
        }
        Ok(pulls)
    }

    /// Maps an unambiguous PR-as-issue row into the portable pull-request shape.
    ///
    /// Forgejo's issue row does not expose branch refs, head/base SHAs,
    /// requested reviewers, or merge records; summary callers must reload an
    /// exact pull request when those detail fields are needed.
    fn materialize_pull_request_summary(&self, repo: &RepoCoord, row: IssueDto) -> PullRequest {
        let merge_state = row
            .pull_request
            .as_ref()
            .and_then(pull_request_marker_merge_state);
        let issue = map_issue(repo, row);
        let repo_id = issue.repo_id.clone();
        let number = issue.number;
        let state = if merge_state.unwrap_or(false) {
            PullRequestState::Merged
        } else if issue.state == temper_forge_model::IssueState::Closed {
            PullRequestState::Closed
        } else {
            PullRequestState::Open
        };

        let mut pull = PullRequest {
            id: format_pull_request_id(repo, number),
            repo_id: repo_id.clone(),
            number,
            title: issue.title,
            body: issue.body,
            state,
            author_id: issue.author_id,
            source: BranchRef {
                repository_id: repo_id.clone(),
                branch: String::new(),
            },
            target: BranchRef {
                repository_id: repo_id,
                branch: String::new(),
            },
            head_sha: None,
            base_sha: None,
            labels: issue.labels,
            assignees: issue.assignees,
            requested_reviewers: Vec::new(),
            dependencies: Vec::new(),
            merge: None,
            version: temper_forge_model::Version::INITIAL,
            created_at: issue.created_at,
            updated_at: issue.updated_at,
            closed_at: issue.closed_at,
        };
        let validator = pull.updated_at.to_rfc3339();
        pull.version = self.versions.observe(pull.id.as_str(), Some(&validator));
        pull
    }
}

fn pr_issue_row_can_answer_summary_query(row: &IssueDto, query: &PullRequestQuery) -> bool {
    // Labelled queries rely on row labels both for the provider-side match and
    // for the returned summary. If a provider ever omits them, fetch exact detail.
    if row.labels.is_none() {
        return false;
    }
    // Assignee filters can only be answered safely when the row carries an
    // assignee collection; otherwise fetch detail rather than guess.
    if query.assignee_id.is_some() && row.assignees.is_none() {
        return false;
    }
    // Forgejo maps both closed and merged PRs to `state=closed`; without a merge
    // marker the summary row cannot distinguish a portable Closed query from a
    // Merged query. Open rows do not need the marker.
    if row.state.eq_ignore_ascii_case("closed") {
        return row
            .pull_request
            .as_ref()
            .and_then(pull_request_marker_merge_state)
            .is_some();
    }
    true
}

fn pull_request_marker_merge_state(marker: &crate::types::PullRequestMarkerDto) -> Option<bool> {
    marker.merged.or_else(|| marker.merged_at.map(|_| true))
}

fn pull_request_state_param(state: Option<PullRequestState>) -> &'static str {
    match state {
        None => "all",
        Some(PullRequestState::Open) => "open",
        Some(PullRequestState::Closed) | Some(PullRequestState::Merged) => "closed",
    }
}

fn pull_matches_query(pull: &PullRequest, query: &PullRequestQuery) -> bool {
    if let Some(state) = query.state {
        if pull.state != state {
            return false;
        }
    }
    if !query
        .labels
        .iter()
        .all(|required| pull.labels.iter().any(|label| label == required))
    {
        return false;
    }
    if let Some(needle) = &query.body_contains {
        if !needle.is_empty() && !pull.body.contains(needle) {
            return false;
        }
    }
    if let Some(author) = &query.author_id {
        if &pull.author_id != author {
            return false;
        }
    }
    if let Some(assignee) = &query.assignee_id {
        if !pull.assignees.iter().any(|candidate| candidate == assignee) {
            return false;
        }
    }
    true
}

fn compare_pull_requests(
    left: &PullRequest,
    right: &PullRequest,
    query: &PullRequestQuery,
) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                ItemSortField::Number => left.number.cmp(&right.number),
                ItemSortField::CreatedAt => left.created_at.cmp(&right.created_at),
                ItemSortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            match sort.direction {
                SortDirection::Asc => comparison,
                SortDirection::Desc => comparison.reverse(),
            }
        })
        .unwrap_or_else(|| left.number.cmp(&right.number));
    primary
        .then_with(|| left.number.cmp(&right.number))
        .then_with(|| left.id.cmp(&right.id))
}
