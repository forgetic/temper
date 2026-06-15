use crate::FilesystemForge;
use crate::lists::sort_pull_requests_by_number;
use crate::metadata::next_timestamp;
use crate::record_ids::merge_commit_sha;
use temper_forge_model::{
    ChangeKind, ForgeError, ForgeResult, MergePullRequest, MergeRecord, PullRequestId,
    PullRequestState,
};

pub(crate) fn merge_pull_request(
    forge: &FilesystemForge,
    id: &PullRequestId,
    input: MergePullRequest,
) -> ForgeResult<MergeRecord> {
    let repo_id = forge
        .find_pull_request_repository_by_id(id)?
        .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

    let mut pull_requests = forge.read_pull_requests_for_existing_repository(&repo_id)?;
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

    let mut metadata = forge.read_metadata()?;
    let merged_by = forge.effective_user(&metadata).id;
    let now = next_timestamp(&mut metadata)?;
    let merge = MergeRecord {
        method: input.method,
        commit_sha: merge_commit_sha(metadata.clock_tick),
        merged_by,
        merged_at: now,
    };

    pull_request.state = PullRequestState::Merged;
    pull_request.merge = Some(merge.clone());
    pull_request.version = pull_request.version.next();
    pull_request.updated_at = now;
    pull_request.closed_at = Some(now);
    let number = pull_request.number;

    sort_pull_requests_by_number(&mut pull_requests);
    forge.write_pull_requests(&repo_id, &pull_requests)?;
    forge.write_metadata(&metadata)?;
    forge.publish_item_hint(&repo_id, number, ChangeKind::PullRequest);

    Ok(merge)
}
