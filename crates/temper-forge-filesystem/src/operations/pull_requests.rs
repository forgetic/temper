use crate::FilesystemForge;
use crate::dependencies::{add_pull_request_dependency, remove_pull_request_dependency};
use crate::lists::{
    apply_assignee_update, apply_label_update, next_pull_request_number, normalize_string_set,
    normalize_user_set, pull_request_matches_query, sort_pull_requests,
    sort_pull_requests_by_number, update_pull_request_state,
};
use crate::metadata::next_timestamp;
use crate::record_ids::pull_request_id;
use crate::validation::check_expected_version;
use temper_forge_model::{
    CandidateLifecycle, ChangeKind, CreatePullRequest, ForgeError, ForgeResult, ItemNumber,
    PullRequest, PullRequestCandidateQuery, PullRequestId, PullRequestQuery, PullRequestState,
    RepositoryId, UpdatePullRequest, Version,
};

pub(crate) fn list_pull_requests(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    query: PullRequestQuery,
) -> ForgeResult<Vec<PullRequest>> {
    forge.require_repository(repo_id)?;

    let mut pull_requests = forge
        .read_pull_requests_for_existing_repository(repo_id)?
        .into_iter()
        .filter(|pull_request| pull_request_matches_query(pull_request, &query))
        .collect::<Vec<_>>();
    if !query.details.dependencies {
        for pull_request in &mut pull_requests {
            pull_request.dependencies.clear();
        }
    }
    sort_pull_requests(&mut pull_requests, &query);
    if let Some(limit) = query.limit {
        pull_requests.truncate(limit);
    }
    Ok(pull_requests)
}

pub(crate) fn list_pull_request_candidates(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    query: PullRequestCandidateQuery,
) -> ForgeResult<Vec<PullRequest>> {
    let labels = query.labels.normalized()?;
    forge.require_repository(repo_id)?;
    let mut pull_requests = forge
        .read_pull_requests_for_existing_repository(repo_id)?
        .into_iter()
        .filter(|pull_request| match query.lifecycle {
            CandidateLifecycle::Open => pull_request.state == PullRequestState::Open,
            CandidateLifecycle::Terminal => matches!(
                pull_request.state,
                PullRequestState::Closed | PullRequestState::Merged
            ),
        })
        .filter(|pull_request| {
            labels.as_ref().is_none_or(|labels| {
                labels
                    .iter()
                    .any(|required| pull_request.labels.iter().any(|label| label == required))
            })
        })
        .collect::<Vec<_>>();
    if !query.details.dependencies {
        for pull_request in &mut pull_requests {
            pull_request.dependencies.clear();
        }
    }
    sort_pull_requests_by_number(&mut pull_requests);
    Ok(pull_requests)
}

pub(crate) fn create_pull_request(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    input: CreatePullRequest,
) -> ForgeResult<PullRequest> {
    let _guard = forge.write_lock()?;
    forge.require_repository(repo_id)?;

    let mut metadata = forge.read_metadata()?;
    let author_id = forge.effective_user(&metadata).id;
    let mut pull_requests = forge.read_pull_requests_for_existing_repository(repo_id)?;
    let number = next_pull_request_number(repo_id, &pull_requests)?;
    let now = next_timestamp(&mut metadata)?;
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
    sort_pull_requests_by_number(&mut pull_requests);
    forge.write_pull_requests(repo_id, &pull_requests)?;
    forge.write_metadata(&metadata)?;
    forge.publish_pull_request_hint(&pull_request, ChangeKind::Created);

    Ok(pull_request)
}

pub(crate) fn get_pull_request_by_number(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    number: ItemNumber,
) -> ForgeResult<Option<PullRequest>> {
    if forge.find_repository_by_id(repo_id)?.is_none() {
        return Ok(None);
    }

    Ok(forge
        .read_pull_requests_for_existing_repository(repo_id)?
        .into_iter()
        .find(|pull_request| pull_request.number == number))
}

pub(crate) fn update_pull_request(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: UpdatePullRequest,
) -> ForgeResult<PullRequest> {
    let _guard = forge.write_lock()?;
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    let mut pull_requests = forge.read_pull_requests_for_existing_repository(&repo_id)?;
    let pull_request = pull_requests
        .iter_mut()
        .find(|pull_request| &pull_request.id == id)
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
    check_expected_version(
        "pull request",
        id,
        input.expected_version,
        pull_request.version,
    )?;

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;

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
    sort_pull_requests_by_number(&mut pull_requests);
    forge.write_pull_requests(&repo_id, &pull_requests)?;
    forge.write_metadata(&metadata)?;
    forge.publish_pull_request_hint(&updated, ChangeKind::Edited);

    Ok(updated)
}

pub(crate) fn add_pull_request_dependency_op(
    forge: &FilesystemForge,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<PullRequest> {
    let _guard = forge.write_lock()?;
    let pull_request = add_pull_request_dependency(forge, id, target)?;
    forge.publish_pull_request_hint(&pull_request, ChangeKind::Dependency);
    Ok(pull_request)
}

pub(crate) fn remove_pull_request_dependency_op(
    forge: &FilesystemForge,
    id: &PullRequestId,
    target: ItemNumber,
) -> ForgeResult<PullRequest> {
    let _guard = forge.write_lock()?;
    let pull_request = remove_pull_request_dependency(forge, id, target)?;
    forge.publish_pull_request_hint(&pull_request, ChangeKind::Dependency);
    Ok(pull_request)
}
