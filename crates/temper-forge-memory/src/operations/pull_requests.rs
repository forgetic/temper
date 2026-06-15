//! Pull-request operations for [`MemoryForge`](crate::MemoryForge).
//!
//! Covers pull-request lifecycle, comments, dependency links, reviewer
//! requests, reviews, and merges.

use crate::MemoryForge;
use crate::dependencies::{add_pull_request_dependency, remove_pull_request_dependency};
use crate::fault::FaultOp;
use crate::ids::{merge_commit_sha, pull_request_comment_id, pull_request_id};
use crate::lists::{
    apply_assignee_update, apply_label_update, normalize_string_set, normalize_user_set,
    pull_request_matches_query, sort_comments, sort_pull_requests, sort_pull_requests_by_number,
    update_pull_request_state,
};
use crate::reviews::{list_reviews, request_reviewers, submit_review};
use crate::util::{check_expected_version, next_comment_number, next_item_number};
use temper_forge_model::{
    ChangeKind, Comment, CreateComment, CreatePullRequest, CreatePullRequestReview, ForgeError,
    ForgeResult, ItemNumber, MergePullRequest, MergeRecord, PullRequest, PullRequestId,
    PullRequestQuery, PullRequestReview, PullRequestState, RepositoryId, RequestReviewers,
    UpdatePullRequest, Version,
};

pub(crate) fn list_pull_requests(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    query: PullRequestQuery,
) -> ForgeResult<Vec<PullRequest>> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::ListPullRequests)?;
    inner.state.require_repository(repo_id)?;
    let mut pull_requests = inner
        .state
        .pull_requests(repo_id)
        .into_iter()
        .filter(|pull_request| pull_request_matches_query(pull_request, &query))
        .collect::<Vec<_>>();
    if !query.details.dependencies {
        for pull_request in &mut pull_requests {
            pull_request.dependencies.clear();
        }
    }
    sort_pull_requests(&mut pull_requests, &query);
    Ok(pull_requests)
}

pub(crate) fn create_pull_request(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    input: CreatePullRequest,
) -> ForgeResult<PullRequest> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::CreatePullRequest)?;
    inner.state.require_repository(repo_id)?;
    let now = inner.state.next_timestamp()?;
    let author_id = forge.effective_user(&inner).id;
    let pull_requests = inner.state.pull_requests_mut(repo_id);
    let number = next_item_number(pull_requests.iter().map(|pr| pr.number))?;
    let pull_request = PullRequest {
        id: pull_request_id(repo_id, number),
        repo_id: repo_id.clone(),
        number,
        title: input.title,
        body: input.body,
        state: PullRequestState::Open,
        author_id,
        source: input.source,
        target: input.target,
        head_sha: None,
        base_sha: None,
        labels: normalize_string_set(input.labels),
        assignees: normalize_user_set(input.assignees),
        requested_reviewers: Vec::new(),
        dependencies: Vec::new(),
        merge: None,
        version: Version::INITIAL,
        created_at: now,
        updated_at: now,
        closed_at: None,
    };
    pull_requests.push(pull_request.clone());
    sort_pull_requests_by_number(pull_requests);
    inner.publish_item_hint(repo_id, pull_request.number, ChangeKind::PullRequest);
    Ok(pull_request)
}

pub(crate) fn get_pull_request(
    forge: &MemoryForge,
    id: &PullRequestId,
) -> ForgeResult<Option<PullRequest>> {
    Ok(forge
        .lock()
        .state
        .find_pull_request(id)
        .map(|(_, pull_request)| pull_request))
}

pub(crate) fn get_pull_request_by_number(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    number: ItemNumber,
) -> ForgeResult<Option<PullRequest>> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::GetPullRequestByNumber)?;
    if !inner.state.repository_exists(repo_id) {
        return Ok(None);
    }
    Ok(inner
        .state
        .pull_requests(repo_id)
        .into_iter()
        .find(|pull_request| pull_request.number == number))
}

pub(crate) fn update_pull_request(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: UpdatePullRequest,
) -> ForgeResult<PullRequest> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::UpdatePullRequest)?;
    let (repo_id, existing) = inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    check_expected_version("pull request", id, input.expected_version, existing.version)?;
    let now = inner.state.next_timestamp()?;
    let pull_requests = inner.state.pull_requests_mut(&repo_id);
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    if let Some(title) = input.title {
        pull_request.title = title;
    }
    if let Some(body) = input.body {
        pull_request.body = body;
    }
    if let Some(state) = input.state {
        update_pull_request_state(pull_request, state, now)?;
    }
    apply_label_update(
        &mut pull_request.labels,
        input.set_labels,
        input.remove_labels,
        input.add_labels,
    );
    apply_assignee_update(
        &mut pull_request.assignees,
        input.remove_assignees,
        input.add_assignees,
    );
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    let updated = pull_request.clone();
    sort_pull_requests_by_number(pull_requests);
    inner.publish_item_hint(&repo_id, updated.number, ChangeKind::PullRequest);
    Ok(updated)
}

pub(crate) fn add_dependency(
    forge: &MemoryForge,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<PullRequest> {
    let mut inner = forge.lock();
    let (pull_request, changed) = add_pull_request_dependency(&mut inner, id, target)?;
    if changed {
        inner.publish_pull_request_hint(&pull_request, ChangeKind::PullRequest);
    }
    Ok(pull_request)
}

pub(crate) fn remove_dependency(
    forge: &MemoryForge,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<PullRequest> {
    let mut inner = forge.lock();
    let (pull_request, changed) = remove_pull_request_dependency(&mut inner, id, target)?;
    if changed {
        inner.publish_pull_request_hint(&pull_request, ChangeKind::PullRequest);
    }
    Ok(pull_request)
}

pub(crate) fn request_pull_request_reviewers(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: RequestReviewers,
) -> ForgeResult<PullRequest> {
    let (pull_request, changed) = request_reviewers(forge, id, input)?;
    if changed {
        forge
            .lock()
            .publish_pull_request_hint(&pull_request, ChangeKind::Review);
    }
    Ok(pull_request)
}

pub(crate) fn list_pull_request_reviews(
    forge: &MemoryForge,
    id: &PullRequestId,
) -> ForgeResult<Vec<PullRequestReview>> {
    list_reviews(forge, id)
}

pub(crate) fn submit_pull_request_review(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: CreatePullRequestReview,
) -> ForgeResult<PullRequestReview> {
    let (review, repo_id, number) = submit_review(forge, id, input)?;
    forge
        .lock()
        .publish_item_hint(&repo_id, number, ChangeKind::Review);
    Ok(review)
}

pub(crate) fn list_comments(forge: &MemoryForge, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
    let inner = forge.lock();
    inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let mut comments = inner.state.pull_request_comments(id);
    sort_comments(&mut comments);
    Ok(comments)
}

pub(crate) fn add_comment(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: CreateComment,
) -> ForgeResult<Comment> {
    let mut inner = forge.lock();
    inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let now = inner.state.next_timestamp()?;
    let author_id = forge.effective_user(&inner).id;
    let comments = inner.state.pull_request_comments_mut(id);
    let number = next_comment_number(comments.len())?;
    let comment = Comment {
        id: pull_request_comment_id(id, number),
        author_id,
        body: input.body,
        created_at: now,
        updated_at: now,
    };
    comments.push(comment.clone());
    sort_comments(comments);
    if let Some((repo_id, pull_request)) = inner.state.find_pull_request(id) {
        inner.publish_item_hint(&repo_id, pull_request.number, ChangeKind::Comment);
    }
    Ok(comment)
}

pub(crate) fn merge_pull_request(
    forge: &MemoryForge,
    id: &PullRequestId,
    input: MergePullRequest,
) -> ForgeResult<MergeRecord> {
    let mut inner = forge.lock();
    inner.faults.take(FaultOp::MergePullRequest)?;
    let (repo_id, _) = inner
        .state
        .find_pull_request(id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    let now = inner.state.next_timestamp()?;
    let clock_tick = inner.state.clock_tick();
    let merged_by = forge.effective_user(&inner).id;
    let pull_requests = inner.state.pull_requests_mut(&repo_id);
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    match pull_request.state {
        PullRequestState::Open => {}
        PullRequestState::Closed => {
            return Err(ForgeError::Conflict(format!("pull request {id} is closed")));
        }
        PullRequestState::Merged => {
            return Err(ForgeError::Conflict(format!("pull request {id} is merged")));
        }
    }

    let merge = MergeRecord {
        method: input.method,
        commit_sha: merge_commit_sha(clock_tick),
        merged_by,
        merged_at: now,
    };
    pull_request.state = PullRequestState::Merged;
    pull_request.merge = Some(merge.clone());
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    let number = pull_request.number;
    pull_request.closed_at = Some(now);
    sort_pull_requests_by_number(pull_requests);
    inner.publish_item_hint(&repo_id, number, ChangeKind::PullRequest);
    Ok(merge)
}
