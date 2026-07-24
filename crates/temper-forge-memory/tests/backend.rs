//! Behaviour tests for the in-memory Forge backend.
//!
//! These exercise the operations the workflow runtime relies on and assert the
//! deterministic ids, logical-clock timestamps, ordering, and the one-shot
//! fault hook. The in-memory futures never park, so a hand-rolled `block_on`
//! drives them to completion without an async runtime.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_forge_model::{
    BranchRef, CandidateLabelSelection, CandidateLifecycle, ChangeSource, ChangeSourceEvent, CiJob,
    CiJobQuery, CiJobStatus, CreateComment, CreateIssue, CreatePullRequest, CreateRepository,
    Forge, ForgeError, IssueCandidateQuery, IssueQuery, IssueState, ItemListDetails, ItemNumber,
    ItemSort, ItemSortField, MergeMethod, MergePullRequest, PullRequestCandidateQuery,
    PullRequestQuery, PullRequestState, PullRequestUpdateState, RepositoryId, RepositoryPath,
    SortDirection, UpdateIssue, UpdatePullRequest, UpsertLabel, UserId, Version,
};

struct NoopWake;
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park"),
    }
}

fn repo_input(owner: &str, name: &str) -> CreateRepository {
    CreateRepository {
        owner: owner.into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(repo_input("acme", "service")))
        .expect("repository created")
        .id
}

fn issue_input(labels: &[&str]) -> CreateIssue {
    CreateIssue {
        title: "code work".into(),
        body: String::new(),
        labels: labels.iter().map(|l| (*l).to_string()).collect(),
        assignees: Vec::<UserId>::new(),
    }
}

fn pr_input(repo: &RepositoryId, labels: &[&str]) -> CreatePullRequest {
    CreatePullRequest {
        title: "implementation".into(),
        body: String::new(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: "feature".into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels: labels.iter().map(|l| (*l).to_string()).collect(),
        assignees: Vec::<UserId>::new(),
    }
}

#[test]
fn successful_mutation_publishes_hint_to_shared_handle() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let mut hints = forge
        .as_user(temper_forge_model::User {
            id: UserId::new("user-observer"),
            handle: "observer".into(),
            display_name: None,
            email: None,
        })
        .subscribe_hints();

    block_on(forge.create_issue(&repo, issue_input(&["code", "ready"]))).expect("issue is created");

    assert!(matches!(
        hints.recv_timeout(Duration::from_millis(10)),
        ChangeSourceEvent::Hint(_)
    ));
}

#[test]
fn rejected_mutation_does_not_publish_hint() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = block_on(forge.create_issue(&repo, issue_input(&["code", "ready"])))
        .expect("issue is created");
    let mut hints = forge.subscribe_hints();

    let error = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            expected_version: Some(Version::new(999)),
            ..UpdateIssue::default()
        },
    ))
    .expect_err("stale version is rejected");

    assert!(matches!(error, ForgeError::Conflict(_)));
    assert_eq!(
        hints.recv_timeout(Duration::from_millis(10)),
        ChangeSourceEvent::Timeout
    );
}

#[test]
fn current_user_is_bootstrapped() {
    let forge = MemoryForge::new();
    let user = block_on(forge.current_user()).expect("current user");
    assert_eq!(user.id, UserId::new("user-1"));
    assert_eq!(user.handle, "local");
    assert_eq!(block_on(forge.get_user(&user.id)).unwrap(), Some(user));
    assert_eq!(
        block_on(forge.get_user(&UserId::new("missing"))).unwrap(),
        None
    );
}

#[test]
fn repositories_use_deterministic_ids_and_reject_duplicate_paths() {
    let forge = MemoryForge::new();
    let first = block_on(forge.create_repository(repo_input("alice", "project"))).unwrap();
    assert_eq!(first.id.as_str(), "repo-0000000000000001");
    assert_eq!(first.created_at, first.updated_at);

    let second = block_on(forge.create_repository(repo_input("alice", "other"))).unwrap();
    assert_eq!(second.id.as_str(), "repo-0000000000000002");

    let duplicate = block_on(forge.create_repository(repo_input("alice", "project")));
    assert!(matches!(duplicate, Err(ForgeError::AlreadyExists(_))));

    assert_eq!(
        block_on(forge.get_repository(&first.id)).unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        block_on(forge.get_repository_by_path(&RepositoryPath::new("alice", "project"))).unwrap(),
        Some(first)
    );
}

#[test]
fn repository_path_lookup_fault_is_one_shot_and_preserves_state() {
    let forge = MemoryForge::new();
    let repository = block_on(forge.create_repository(repo_input("alice", "project"))).unwrap();
    let path = RepositoryPath::new("alice", "project");

    forge.fail_next(
        FaultOp::GetRepositoryByPath,
        "simulated repository lookup failure",
    );
    let failed = block_on(forge.get_repository_by_path(&path));
    assert!(matches!(
        failed,
        Err(ForgeError::Backend(message)) if message.contains("repository lookup failure")
    ));

    assert_eq!(
        block_on(forge.get_repository(&repository.id)).unwrap(),
        Some(repository.clone())
    );
    assert_eq!(
        block_on(forge.get_repository_by_path(&path)).unwrap(),
        Some(repository)
    );
}

#[test]
fn empty_repository_fields_are_rejected() {
    let forge = MemoryForge::new();
    assert!(matches!(
        block_on(forge.create_repository(repo_input("", "name"))),
        Err(ForgeError::InvalidRequest(_))
    ));
}

#[test]
fn labels_upsert_and_sort_by_name() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);

    block_on(forge.upsert_label(
        &repo,
        UpsertLabel {
            name: "ready".into(),
            color: Some("#fff".into()),
            description: None,
        },
    ))
    .unwrap();
    let updated = block_on(forge.upsert_label(
        &repo,
        UpsertLabel {
            name: "ready".into(),
            color: Some("#000".into()),
            description: Some("ready to work".into()),
        },
    ))
    .unwrap();
    assert_eq!(updated.color.as_deref(), Some("#000"));

    block_on(forge.upsert_label(
        &repo,
        UpsertLabel {
            name: "code".into(),
            color: None,
            description: None,
        },
    ))
    .unwrap();

    let names: Vec<String> = block_on(forge.list_labels(&repo))
        .unwrap()
        .into_iter()
        .map(|label| label.name)
        .collect();
    assert_eq!(names, vec!["code".to_string(), "ready".to_string()]);

    assert!(matches!(
        block_on(forge.list_labels(&RepositoryId::new("repo-missing"))),
        Err(ForgeError::NotFound(_))
    ));
}

#[test]
fn issues_create_list_and_update_labels() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);

    let first = block_on(forge.create_issue(&repo, issue_input(&["code", "ready"]))).unwrap();
    assert_eq!(first.number, ItemNumber::new(1));
    assert_eq!(
        first.id.as_str(),
        "issue-repo-0000000000000001-0000000000000001"
    );
    assert_eq!(first.state, IssueState::Open);
    assert_eq!(first.labels, vec!["code".to_string(), "ready".to_string()]);

    let second = block_on(forge.create_issue(&repo, issue_input(&["docs"]))).unwrap();
    assert_eq!(second.number, ItemNumber::new(2));

    let updated = block_on(forge.update_issue(
        &first.id,
        UpdateIssue {
            add_labels: vec!["in-progress".into()],
            remove_labels: vec!["ready".into()],
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(
        updated.labels,
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert!(updated.updated_at > first.updated_at);

    let open = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            labels: vec!["code".into()],
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, first.id);

    let closed = block_on(forge.update_issue(
        &first.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(closed.state, IssueState::Closed);
    assert!(closed.closed_at.is_some());
}

#[test]
fn candidate_reads_are_any_label_while_ordinary_queries_remain_conjunctive() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let both = block_on(forge.create_issue(&repo, issue_input(&["code", "ready"]))).unwrap();
    let code = block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();
    let unrelated = block_on(forge.create_issue(&repo, issue_input(&["docs"]))).unwrap();

    let conjunctive = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            state: Some(IssueState::Open),
            labels: vec!["code".into(), "ready".into()],
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        conjunctive
            .iter()
            .map(|issue| issue.id.clone())
            .collect::<Vec<_>>(),
        vec![both.id.clone()]
    );

    let candidates = block_on(forge.list_issue_candidates(
        &repo,
        IssueCandidateQuery {
            lifecycle: CandidateLifecycle::Open,
            labels: CandidateLabelSelection::AnyOf(vec![
                "ready".into(),
                "code".into(),
                "ready".into(),
            ]),
            ..IssueCandidateQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|issue| issue.number)
            .collect::<Vec<_>>(),
        vec![both.number, code.number]
    );
    assert!(!candidates.iter().any(|issue| issue.id == unrelated.id));
    assert!(candidates.iter().all(|issue| issue.dependencies.is_empty()));
}

#[test]
fn terminal_pull_candidate_bucket_covers_closed_and_merged_without_type_collisions() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = block_on(forge.create_issue(&repo, issue_input(&["landed"]))).unwrap();
    let closed = block_on(forge.create_pull_request(&repo, pr_input(&repo, &["landed"]))).unwrap();
    let merged = block_on(forge.create_pull_request(&repo, pr_input(&repo, &["landed"]))).unwrap();
    assert_eq!(
        issue.number, closed.number,
        "issue and PR numbers collide by type"
    );

    block_on(forge.update_pull_request(
        &closed.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    block_on(forge.merge_pull_request(
        &merged.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .unwrap();

    let candidates = block_on(forge.list_pull_request_candidates(
        &repo,
        PullRequestCandidateQuery {
            lifecycle: CandidateLifecycle::Terminal,
            labels: CandidateLabelSelection::AnyOf(vec!["landed".into()]),
            ..PullRequestCandidateQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|pull_request| (pull_request.number, pull_request.state))
            .collect::<Vec<_>>(),
        vec![
            (closed.number, PullRequestState::Closed),
            (merged.number, PullRequestState::Merged),
        ]
    );
}

#[test]
fn issue_comments_are_numbered_and_ordered() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = block_on(forge.create_issue(&repo, issue_input(&[]))).unwrap();

    let first =
        block_on(forge.add_issue_comment(&issue.id, CreateComment { body: "one".into() })).unwrap();
    let second =
        block_on(forge.add_issue_comment(&issue.id, CreateComment { body: "two".into() })).unwrap();
    assert_eq!(
        first.id.as_str(),
        "comment-issue-repo-0000000000000001-0000000000000001-0000000000000001"
    );
    assert_ne!(first.id, second.id);

    let comments = block_on(forge.list_issue_comments(&issue.id)).unwrap();
    assert_eq!(comments.len(), 2);
    assert!(comments[0].created_at <= comments[1].created_at);

    assert!(matches!(
        block_on(forge.list_issue_comments(&temper_forge_model::IssueId::new("issue-missing"))),
        Err(ForgeError::NotFound(_))
    ));
}

#[test]
fn conditional_update_rejects_a_stale_version() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();
    // A freshly created artifact starts at the initial version.
    assert_eq!(issue.version, Version::INITIAL);

    // A compare-and-swap against the captured version succeeds and advances it.
    let updated = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            add_labels: vec!["in-progress".into()],
            expected_version: Some(issue.version),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.version, issue.version.next());

    // A second update against the now-stale captured version is rejected as a
    // conflict without mutating anything.
    let conflict = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            add_labels: vec!["ready".into()],
            expected_version: Some(issue.version),
            ..UpdateIssue::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(conflict, ForgeError::Conflict(_)));

    let current = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    assert_eq!(
        current.version, updated.version,
        "a rejected CAS leaves the version untouched"
    );
    assert!(
        !current.labels.contains(&"ready".to_string()),
        "a rejected CAS applies no labels"
    );

    // An unconditional update (no precondition) still applies and advances the version.
    let unconditional = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            add_labels: vec!["ready".into()],
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(unconditional.version, updated.version.next());
    assert!(unconditional.labels.contains(&"ready".to_string()));

    // Conditional updates work on pull requests too, and merging advances the version.
    let pr = block_on(forge.create_pull_request(&repo, pr_input(&repo, &["impl"]))).unwrap();
    assert_eq!(pr.version, Version::INITIAL);
    let pr_conflict = block_on(forge.update_pull_request(
        &pr.id,
        UpdatePullRequest {
            add_labels: vec!["needs-review".into()],
            expected_version: Some(Version::new(99)),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap_err();
    assert!(matches!(pr_conflict, ForgeError::Conflict(_)));
    let merged = block_on(forge.merge_pull_request(
        &pr.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ));
    assert!(merged.is_ok());
    let after_merge = block_on(forge.get_pull_request(&pr.id)).unwrap().unwrap();
    assert_eq!(
        after_merge.version,
        pr.version.next(),
        "a merge advances the version"
    );
}

#[test]
fn pull_requests_create_update_and_merge() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let pr = block_on(forge.create_pull_request(&repo, pr_input(&repo, &["impl"]))).unwrap();
    assert_eq!(pr.state, PullRequestState::Open);
    assert_eq!(
        pr.id.as_str(),
        "pull-request-repo-0000000000000001-0000000000000001"
    );

    block_on(forge.update_pull_request(
        &pr.id,
        UpdatePullRequest {
            add_labels: vec!["needs-review".into()],
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();

    let merge = block_on(forge.merge_pull_request(
        &pr.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .unwrap();
    assert_eq!(merge.method, MergeMethod::Squash);
    assert_eq!(merge.commit_sha.len(), 40);

    let merged = block_on(forge.get_pull_request(&pr.id)).unwrap().unwrap();
    assert_eq!(merged.state, PullRequestState::Merged);
    assert!(merged.merge.is_some());

    // Re-merging a merged pull request conflicts.
    assert!(matches!(
        block_on(forge.merge_pull_request(
            &pr.id,
            MergePullRequest {
                method: MergeMethod::MergeCommit,
                commit_title: None,
                commit_body: None,
                delete_source_branch: false,
            },
        )),
        Err(ForgeError::Conflict(_))
    ));
}

#[test]
fn list_limits_apply_after_deterministic_sorting() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    for _ in 0..3 {
        block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();
        block_on(forge.create_pull_request(&repo, pr_input(&repo, &["code"]))).unwrap();
    }

    let issues = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            limit: Some(2),
            sort: Some(ItemSort {
                field: ItemSortField::Number,
                direction: SortDirection::Desc,
            }),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        issues
            .iter()
            .map(|item| item.number.get())
            .collect::<Vec<_>>(),
        vec![3, 2]
    );

    let pulls = block_on(forge.list_pull_requests(
        &repo,
        PullRequestQuery {
            limit: Some(0),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert!(pulls.is_empty());
}

#[test]
fn dependency_links_are_set_like_and_deterministic() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let source = block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();
    let first_target = block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();
    let second_target = block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();

    let with_second = block_on(forge.add_issue_dependency(&source.id, second_target.number))
        .expect("dependency added");
    assert_eq!(with_second.dependencies, vec![second_target.number]);

    let with_both = block_on(forge.add_issue_dependency(&source.id, first_target.number))
        .expect("dependency added and sorted");
    assert_eq!(
        with_both.dependencies,
        vec![first_target.number, second_target.number]
    );
    let duplicate = block_on(forge.add_issue_dependency(&source.id, first_target.number))
        .expect("duplicate add is a no-op");
    assert_eq!(duplicate.dependencies, with_both.dependencies);
    assert_eq!(duplicate.version, with_both.version);

    let removed = block_on(forge.remove_issue_dependency(&source.id, first_target.number))
        .expect("dependency removed");
    assert_eq!(removed.dependencies, vec![second_target.number]);
    let removed_again = block_on(forge.remove_issue_dependency(&source.id, first_target.number))
        .expect("duplicate remove is a no-op");
    assert_eq!(removed_again.dependencies, removed.dependencies);
    assert_eq!(removed_again.version, removed.version);

    let listed = block_on(forge.list_issues(&repo, IssueQuery::default())).unwrap();
    assert_eq!(listed[0].dependencies, vec![second_target.number]);
    let summaries = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            details: ItemListDetails::summary(),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert!(summaries[0].dependencies.is_empty());

    let pr = block_on(forge.create_pull_request(&repo, pr_input(&repo, &["implementation"])))
        .expect("pull request created");
    let pr_with_dependency =
        block_on(forge.add_pull_request_dependency(&pr.id, second_target.number))
            .expect("pull request dependency added");
    assert_eq!(pr_with_dependency.dependencies, vec![second_target.number]);
    let pr_summaries = block_on(forge.list_pull_requests(
        &repo,
        PullRequestQuery {
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert!(pr_summaries[0].dependencies.is_empty());
    let pr_without_dependency =
        block_on(forge.remove_pull_request_dependency(&pr.id, second_target.number))
            .expect("pull request dependency removed");
    assert!(pr_without_dependency.dependencies.is_empty());

    let missing =
        block_on(forge.add_issue_dependency(&source.id, ItemNumber::new(999))).unwrap_err();
    assert!(matches!(missing, ForgeError::NotFound(_)));
}

#[test]
fn ci_jobs_can_be_seeded_filtered_and_looked_up() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp(10, 0).unwrap();
    let job = CiJob {
        id: temper_forge_model::CiJobId::new("ci-1"),
        repo_id: repo.clone(),
        pull_request_id: None,
        commit_sha: "abc123".into(),
        name: "build".into(),
        status: CiJobStatus::Completed,
        conclusion: Some(temper_forge_model::CiJobConclusion::StartupFailure),
        provider_conclusion: Some("startup_failure".into()),
        provider_reason: Some("runner failed to initialize".into()),
        run_id: Some("run-9".into()),
        attempt: Some("2".into()),
        url: None,
        created_at: now,
        started_at: Some(now),
        completed_at: Some(now),
        updated_at: now,
    };
    forge.seed_ci_jobs(&repo, vec![job.clone()]);

    let matching = block_on(forge.list_ci_jobs(
        &repo,
        CiJobQuery {
            status: Some(CiJobStatus::Completed),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(matching.len(), 1);

    let none = block_on(forge.list_ci_jobs(
        &repo,
        CiJobQuery {
            status: Some(CiJobStatus::Running),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert!(none.is_empty());

    assert_eq!(
        block_on(forge.get_ci_job(&temper_forge_model::CiJobId::new("ci-1"))).unwrap(),
        Some(job)
    );
}

#[test]
fn fault_hook_forces_one_shot_backend_errors() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = block_on(forge.create_issue(&repo, issue_input(&["code"]))).unwrap();

    forge.fail_next(FaultOp::GetIssueByNumber, "simulated unreachable backend");
    let failed = block_on(forge.get_issue_by_number(&repo, issue.number));
    assert!(matches!(failed, Err(ForgeError::Backend(message)) if message.contains("unreachable")));

    // The fault is one-shot: the next call succeeds.
    let recovered = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .expect("issue still exists");
    assert_eq!(recovered.id, issue.id);

    // Cleared faults do not fire.
    forge.fail_next(FaultOp::ListIssues, "boom");
    forge.clear_faults();
    assert!(block_on(forge.list_issues(&repo, IssueQuery::default())).is_ok());
}

#[test]
fn cloned_handles_share_one_store() {
    let forge = MemoryForge::new();
    let clone = forge.clone();
    let repo = new_repo(&forge);
    // The clone observes the repository created through the original handle.
    assert!(block_on(clone.get_repository(&repo)).unwrap().is_some());
}
