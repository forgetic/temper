//! Pull-request list path for GitHub: state filter, client-side filtering, and
//! sorting of `GET /repos/{owner}/{repo}/pulls`.

use super::*;
use crate::types::PullRequestDto;
use std::cmp::Ordering;
use temper_forge::{ItemSortField, PullRequestQuery, PullRequestState, SortDirection};

impl<C: HttpClient> GitHubForge<C> {
    /// Lists pull requests in a repository, filtered and sorted per `query`.
    ///
    /// GitHub's `/pulls` list rows already carry labels, assignees, and
    /// requested reviewers, so one provider-side state filter suffices and the
    /// remaining filters (labels, body, author, assignee) are applied
    /// client-side. List rows omit the `merged` boolean but include
    /// `merged_at`, which the mapping uses to distinguish merged from closed.
    pub async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        let path = format!("/repos/{}/pulls", repo.path_segment());
        let base_query = vec![(
            "state".to_string(),
            pull_request_state_param(query.state).to_string(),
        )];
        let dtos: Vec<PullRequestDto> = self
            .list_all("list pull requests", &path, base_query)
            .await?;
        let mut pulls: Vec<PullRequest> = dtos
            .into_iter()
            .map(|dto| self.materialize_pull_request(&repo, dto, None))
            .collect();
        pulls.retain(|pull| pull_matches_query(pull, &query));
        // GitHub exposes no native dependency links, so `query.details` needs
        // no enrichment pass: `dependencies` is always empty.
        pulls.sort_by(|left, right| compare_pull_requests(left, right, &query));
        Ok(pulls)
    }
}

fn pull_request_state_param(state: Option<PullRequestState>) -> &'static str {
    match state {
        None => "all",
        Some(PullRequestState::Open) => "open",
        // GitHub maps both portable Closed and Merged to provider `closed`;
        // the client-side filter separates them after mapping.
        Some(PullRequestState::Closed) | Some(PullRequestState::Merged) => "closed",
    }
}

fn pull_matches_query(pull: &PullRequest, query: &PullRequestQuery) -> bool {
    if let Some(state) = query.state
        && pull.state != state
    {
        return false;
    }
    if !query
        .labels
        .iter()
        .all(|required| pull.labels.iter().any(|label| label == required))
    {
        return false;
    }
    if let Some(needle) = &query.body_contains
        && !needle.is_empty()
        && !pull.body.contains(needle)
    {
        return false;
    }
    if let Some(author) = &query.author_id
        && &pull.author_id != author
    {
        return false;
    }
    if let Some(assignee) = &query.assignee_id
        && !pull.assignees.iter().any(|candidate| candidate == assignee)
    {
        return false;
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
