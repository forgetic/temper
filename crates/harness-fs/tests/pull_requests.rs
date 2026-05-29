mod support;

use harness_forge::{
    CiJobId, CiJobQuery, Forge, ForgeError, ItemNumber, ItemSort, ItemSortField, MergeMethod,
    MergePullRequest, PullRequest, PullRequestId, PullRequestQuery, PullRequestState,
    PullRequestUpdateState, RepositoryId, SortDirection, UpdatePullRequest, UserId,
};
use support::{
    block_on, branch, comment, pull_request, pull_request_with, repository, user_ids, TestRoot,
};

fn pull_request_titles(pull_requests: &[PullRequest]) -> Vec<String> {
    pull_requests
        .iter()
        .map(|pull_request| pull_request.title.clone())
        .collect()
}

#[test]
fn pull_requests_are_empty_for_new_repository() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    assert_eq!(
        block_on(forge.list_pull_requests(&repository.id, PullRequestQuery::default())).unwrap(),
        Vec::new()
    );
}

#[test]
fn pull_requests_can_be_created_with_deterministic_identity_and_author() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    let created = block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(
            &repository.id,
            "Add login flow",
            "Implements the login flow.",
            &["triage", "enhancement", "triage"],
            &["user-2", "user-1", "user-2"],
        ),
    ))
    .unwrap();

    assert_eq!(
        created.id.as_str(),
        "pull-request-repo-0000000000000001-0000000000000001"
    );
    assert_eq!(created.repo_id, repository.id);
    assert_eq!(created.number, ItemNumber::new(1));
    assert_eq!(created.title, "Add login flow");
    assert_eq!(created.body, "Implements the login flow.");
    assert_eq!(created.state, PullRequestState::Open);
    assert_eq!(created.author_id, UserId::new("user-1"));
    assert_eq!(created.source, branch(&repository.id, "feature"));
    assert_eq!(created.target, branch(&repository.id, "main"));
    assert_eq!(created.head_sha, None);
    assert_eq!(created.base_sha, None);
    assert_eq!(created.labels, vec!["enhancement", "triage"]);
    assert_eq!(created.assignees, user_ids(&["user-1", "user-2"]));
    assert_eq!(created.merge, None);
    assert_eq!(created.created_at, created.updated_at);
    assert_eq!(created.created_at.timestamp(), 2);
    assert_eq!(created.closed_at, None);
}

#[test]
fn pull_requests_can_be_looked_up_by_id() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let first_repository = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second_repository =
        block_on(forge.create_repository(repository("alice", "second"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &second_repository.id,
        pull_request(&second_repository.id, "Second repo change"),
    ))
    .unwrap();

    assert_eq!(
        block_on(forge.get_pull_request(&pull_request.id)).unwrap(),
        Some(pull_request)
    );
    assert_eq!(
        block_on(forge.get_pull_request(&PullRequestId::new(
            "pull-request-repo-0000000000000001-0000000000009999"
        )))
        .unwrap(),
        None
    );
    assert_eq!(
        block_on(forge.list_pull_requests(&first_repository.id, PullRequestQuery::default()))
            .unwrap(),
        Vec::new()
    );
}

#[test]
fn pull_requests_can_be_looked_up_by_repository_scoped_number() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let first =
        block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "First")))
            .unwrap();
    let second =
        block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "Second")))
            .unwrap();

    assert_eq!(
        block_on(forge.get_pull_request_by_number(&repository.id, ItemNumber::new(1))).unwrap(),
        Some(first)
    );
    assert_eq!(
        block_on(forge.get_pull_request_by_number(&repository.id, ItemNumber::new(2))).unwrap(),
        Some(second)
    );
    assert_eq!(
        block_on(forge.get_pull_request_by_number(&repository.id, ItemNumber::new(999))).unwrap(),
        None
    );
    assert_eq!(
        block_on(forge.get_pull_request_by_number(
            &RepositoryId::new("repo-0000000000009999"),
            ItemNumber::new(1),
        ))
        .unwrap(),
        None
    );
}

#[test]
fn pull_requests_are_persisted_and_reopened() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let first = block_on(
        forge.create_pull_request(&repository.id, pull_request(&repository.id, "Persist me")),
    )
    .unwrap();
    let second = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Persist me too"),
    ))
    .unwrap();

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.list_pull_requests(&repository.id, PullRequestQuery::default())).unwrap(),
        vec![first, second]
    );
}

#[test]
fn pull_request_operations_handle_missing_repository_targets() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let missing = RepositoryId::new("repo-0000000000009999");

    let list_error =
        block_on(forge.list_pull_requests(&missing, PullRequestQuery::default())).unwrap_err();
    assert!(matches!(
        list_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));

    let create_error =
        block_on(forge.create_pull_request(&missing, pull_request(&missing, "Missing repo")))
            .unwrap_err();
    assert!(matches!(
        create_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));

    assert_eq!(
        block_on(forge.get_pull_request_by_number(&missing, ItemNumber::new(1))).unwrap(),
        None
    );
}

#[test]
fn updating_missing_pull_request_returns_not_found() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();

    let error = block_on(forge.update_pull_request(
        &PullRequestId::new("pull-request-repo-0000000000000001-0000000000009999"),
        UpdatePullRequest {
            title: Some("New title".into()),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        ForgeError::NotFound(message)
            if message == "pull request pull-request-repo-0000000000000001-0000000000009999"
    ));
}

#[test]
fn pull_request_lists_filter_by_state_labels_author_and_assignee() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let open_bug = block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(
            &repository.id,
            "Open bug fix",
            "",
            &["bug", "urgent"],
            &["user-2"],
        ),
    ))
    .unwrap();
    let docs = block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(&repository.id, "Docs", "", &["docs"], &["user-3"]),
    ))
    .unwrap();
    let closed_bug = block_on(forge.create_pull_request(
        &repository.id,
        pull_request_with(
            &repository.id,
            "Closed bug fix",
            "",
            &["bug", "urgent", "backend"],
            &["user-3"],
        ),
    ))
    .unwrap();
    block_on(forge.update_pull_request(
        &closed_bug.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();

    let open = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            state: Some(PullRequestState::Open),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(pull_request_titles(&open), vec!["Open bug fix", "Docs"]);

    let closed = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            state: Some(PullRequestState::Closed),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(pull_request_titles(&closed), vec!["Closed bug fix"]);

    let merged = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            state: Some(PullRequestState::Merged),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(merged, Vec::new());

    let bug_and_urgent = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            labels: vec!["bug".into(), "urgent".into()],
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        pull_request_titles(&bug_and_urgent),
        vec!["Open bug fix", "Closed bug fix"]
    );

    let bug_and_missing = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            labels: vec!["bug".into(), "missing".into()],
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(bug_and_missing, Vec::new());

    let current_author = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            author_id: Some(UserId::new("user-1")),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        pull_request_titles(&current_author),
        vec!["Open bug fix", "Docs", "Closed bug fix"]
    );

    let missing_author = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            author_id: Some(UserId::new("missing")),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(missing_author, Vec::new());

    let assigned_to_user_3 = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            assignee_id: Some(UserId::new("user-3")),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        pull_request_titles(&assigned_to_user_3),
        vec!["Docs", "Closed bug fix"]
    );

    assert_eq!(open_bug.number, ItemNumber::new(1));
    assert_eq!(docs.number, ItemNumber::new(2));
}

#[test]
fn pull_request_lists_sort_deterministically() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let first =
        block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "First")))
            .unwrap();
    block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "Second")))
        .unwrap();
    block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "Third")))
        .unwrap();

    let default =
        block_on(forge.list_pull_requests(&repository.id, PullRequestQuery::default())).unwrap();
    assert_eq!(
        pull_request_titles(&default),
        vec!["First", "Second", "Third"]
    );

    let number_desc = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            sort: Some(ItemSort {
                field: ItemSortField::Number,
                direction: SortDirection::Desc,
            }),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        pull_request_titles(&number_desc),
        vec!["Third", "Second", "First"]
    );

    let created_desc = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            sort: Some(ItemSort {
                field: ItemSortField::CreatedAt,
                direction: SortDirection::Desc,
            }),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        pull_request_titles(&created_desc),
        vec!["Third", "Second", "First"]
    );

    block_on(forge.update_pull_request(
        &first.id,
        UpdatePullRequest {
            title: Some("First updated".into()),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    let updated_desc = block_on(forge.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            sort: Some(ItemSort {
                field: ItemSortField::UpdatedAt,
                direction: SortDirection::Desc,
            }),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        pull_request_titles(&updated_desc),
        vec!["First updated", "Third", "Second"]
    );
}

#[test]
fn pull_requests_are_scoped_to_their_repository() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let first_repository = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second_repository =
        block_on(forge.create_repository(repository("alice", "second"))).unwrap();

    let first_pull_request = block_on(forge.create_pull_request(
        &first_repository.id,
        pull_request(&first_repository.id, "First repo change"),
    ))
    .unwrap();
    let second_pull_request = block_on(forge.create_pull_request(
        &second_repository.id,
        pull_request(&second_repository.id, "Second repo change"),
    ))
    .unwrap();

    assert_ne!(first_pull_request.id, second_pull_request.id);
    assert_eq!(first_pull_request.number, ItemNumber::new(1));
    assert_eq!(second_pull_request.number, ItemNumber::new(1));
    assert_eq!(first_pull_request.repo_id, first_repository.id);
    assert_eq!(second_pull_request.repo_id, second_repository.id);
    assert_eq!(
        pull_request_titles(
            &block_on(forge.list_pull_requests(&first_repository.id, PullRequestQuery::default()))
                .unwrap()
        ),
        vec!["First repo change"]
    );
    assert_eq!(
        pull_request_titles(
            &block_on(forge.list_pull_requests(&second_repository.id, PullRequestQuery::default()))
                .unwrap()
        ),
        vec!["Second repo change"]
    );
}

#[test]
fn pull_request_close_and_reopen_updates_closed_at_deterministically() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(
        forge.create_pull_request(&repository.id, pull_request(&repository.id, "Lifecycle")),
    )
    .unwrap();

    let closed = block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    assert_eq!(closed.state, PullRequestState::Closed);
    assert_eq!(closed.closed_at, Some(closed.updated_at));
    assert!(closed.updated_at > pull_request.updated_at);

    let reopened = block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Open),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    assert_eq!(reopened.state, PullRequestState::Open);
    assert_eq!(reopened.closed_at, None);
    assert!(reopened.updated_at > closed.updated_at);

    let closed_again = block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    assert_eq!(closed_again.closed_at, Some(closed_again.updated_at));
    assert!(closed_again.updated_at > reopened.updated_at);
}

#[test]
fn pull_request_comments_merges_and_ci_jobs_remain_unsupported() {
    let root = TestRoot::new("pull-requests");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Unsupported operations"),
    ))
    .unwrap();

    let list_comments_error =
        block_on(forge.list_pull_request_comments(&pull_request.id)).unwrap_err();
    assert!(matches!(
        list_comments_error,
        ForgeError::InvalidRequest(message)
            if message == "filesystem backend does not support list_pull_request_comments yet"
    ));

    let add_comment_error =
        block_on(forge.add_pull_request_comment(&pull_request.id, comment("Still unsupported")))
            .unwrap_err();
    assert!(matches!(
        add_comment_error,
        ForgeError::InvalidRequest(message)
            if message == "filesystem backend does not support add_pull_request_comment yet"
    ));

    let merge_error = block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
        },
    ))
    .unwrap_err();
    assert!(matches!(
        merge_error,
        ForgeError::InvalidRequest(message)
            if message == "filesystem backend does not support merge_pull_request yet"
    ));

    assert_eq!(
        block_on(forge.get_pull_request(&pull_request.id))
            .unwrap()
            .unwrap()
            .state,
        PullRequestState::Open
    );

    let list_ci_error =
        block_on(forge.list_ci_jobs(&repository.id, CiJobQuery::default())).unwrap_err();
    assert!(matches!(
        list_ci_error,
        ForgeError::InvalidRequest(message)
            if message == "filesystem backend does not support list_ci_jobs yet"
    ));

    let get_ci_error = block_on(forge.get_ci_job(&CiJobId::new("ci-job-1"))).unwrap_err();
    assert!(matches!(
        get_ci_error,
        ForgeError::InvalidRequest(message)
            if message == "filesystem backend does not support get_ci_job yet"
    ));
}
