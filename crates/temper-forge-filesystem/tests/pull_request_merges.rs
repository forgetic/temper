mod support;

use support::{TestRoot, block_on, pull_request, repository};
use temper_forge_model::{
    Forge, ForgeError, MergeMethod, MergePullRequest, PullRequestQuery, PullRequestState,
    PullRequestUpdateState, UpdatePullRequest, UserId,
};

fn merge_input(method: MergeMethod) -> MergePullRequest {
    MergePullRequest {
        method,
        commit_title: None,
        commit_body: None,
    }
}

#[test]
fn pull_request_can_be_merged_with_deterministic_record_and_state() {
    let root = TestRoot::new("pull-request-merges");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Ready to merge"),
    ))
    .unwrap();

    let merge = block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: Some("Squash title".into()),
            commit_body: Some("Squash body".into()),
        },
    ))
    .unwrap();

    assert_eq!(merge.method, MergeMethod::Squash);
    assert_eq!(merge.commit_sha, "0000000000000000000000000000000000000003");
    assert_eq!(merge.merged_by, UserId::new("user-1"));
    assert_eq!(merge.merged_at.timestamp(), 3);

    let merged = block_on(forge.get_pull_request(&pull_request.id))
        .unwrap()
        .unwrap();
    assert_eq!(merged.state, PullRequestState::Merged);
    assert_eq!(merged.merge, Some(merge.clone()));
    assert_eq!(merged.updated_at, merge.merged_at);
    assert_eq!(merged.closed_at, Some(merge.merged_at));

    assert_eq!(
        block_on(forge.list_pull_requests(
            &repository.id,
            PullRequestQuery {
                state: Some(PullRequestState::Merged),
                ..PullRequestQuery::default()
            },
        ))
        .unwrap(),
        vec![merged]
    );
}

#[test]
fn merge_records_requested_method_for_each_supported_method() {
    let root = TestRoot::new("pull-request-merges");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    for method in [
        MergeMethod::MergeCommit,
        MergeMethod::Squash,
        MergeMethod::Rebase,
    ] {
        let pull_request = block_on(
            forge.create_pull_request(&repository.id, pull_request(&repository.id, "Method check")),
        )
        .unwrap();
        let merge =
            block_on(forge.merge_pull_request(&pull_request.id, merge_input(method))).unwrap();

        assert_eq!(merge.method, method);
    }
}

#[test]
fn merged_pull_request_is_persisted_and_reopened() {
    let root = TestRoot::new("pull-request-merges");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Persist merge"),
    ))
    .unwrap();
    let merge =
        block_on(forge.merge_pull_request(&pull_request.id, merge_input(MergeMethod::MergeCommit)))
            .unwrap();

    let reopened = root.forge();
    let persisted = block_on(reopened.get_pull_request(&pull_request.id))
        .unwrap()
        .unwrap();
    assert_eq!(persisted.state, PullRequestState::Merged);
    assert_eq!(persisted.merge, Some(merge));
}

#[test]
fn merging_missing_pull_request_returns_not_found() {
    let root = TestRoot::new("pull-request-merges");
    let forge = root.forge();

    let error = block_on(forge.merge_pull_request(
        &temper_forge_model::PullRequestId::new(
            "pull-request-repo-0000000000000001-0000000000009999",
        ),
        merge_input(MergeMethod::Squash),
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        ForgeError::NotFound(message)
            if message == "pull request pull-request-repo-0000000000000001-0000000000009999"
    ));
}

#[test]
fn closed_or_already_merged_pull_request_cannot_be_merged() {
    let root = TestRoot::new("pull-request-merges");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let closed =
        block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "Closed")))
            .unwrap();
    block_on(forge.update_pull_request(
        &closed.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();

    let error = block_on(forge.merge_pull_request(&closed.id, merge_input(MergeMethod::Squash)))
        .unwrap_err();
    assert!(matches!(
        error,
        ForgeError::Conflict(message) if message == format!("pull request {} is closed", closed.id)
    ));

    let merged =
        block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "Merged")))
            .unwrap();
    block_on(forge.merge_pull_request(&merged.id, merge_input(MergeMethod::Squash))).unwrap();

    let error = block_on(forge.merge_pull_request(&merged.id, merge_input(MergeMethod::Squash)))
        .unwrap_err();
    assert!(matches!(
        error,
        ForgeError::Conflict(message) if message == format!("pull request {} is merged", merged.id)
    ));
}
