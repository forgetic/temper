// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

pub(crate) async fn issue_labels(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists")
        .labels
}

pub(crate) async fn list_issues(forge: &MemoryForge, repo: &RepositoryId) -> Vec<Issue> {
    forge
        .list_issues(repo, IssueQuery::default())
        .await
        .expect("list issues succeeds")
}

pub(crate) async fn wait_for_issue_count(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) -> Vec<Issue> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let issues = list_issues(forge, repo).await;
        if issues.len() == expected {
            return issues;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} issue(s), saw {}",
            issues.len()
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) async fn assert_issue_count_stays(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let issues = list_issues(forge, repo).await;
        assert_eq!(issues.len(), expected);
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) fn issue_by_slug<'a>(issues: &'a [Issue], slug: &str) -> &'a Issue {
    issues
        .iter()
        .find(|issue| {
            parse_metadata_block(&issue.body)
                .expect("issue metadata parses")
                .is_some_and(|metadata| {
                    metadata
                        .correlation_key
                        .as_deref()
                        .is_some_and(|key| key.contains(&format!("/child:{}:{slug}", slug.len())))
                })
        })
        .unwrap_or_else(|| panic!("issue for child slug {slug:?} exists"))
}

pub(crate) async fn pull_request_labels(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request reload succeeds")
        .expect("pull request exists")
        .labels
}

pub(crate) async fn pull_request_reviews(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<PullRequestReview> {
    let pull_request = forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request reload succeeds")
        .expect("pull request exists");
    forge
        .list_pull_request_reviews(&pull_request.id)
        .await
        .expect("list pull request reviews succeeds")
}

pub(crate) async fn pull_request_labels_and_reviews(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> (Vec<String>, Vec<PullRequestReview>) {
    (
        pull_request_labels(forge, repo, number).await,
        pull_request_reviews(forge, repo, number).await,
    )
}

pub(crate) async fn issue_comment_bodies(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    forge
        .list_issue_comments(&issue.id)
        .await
        .expect("list issue comments succeeds")
        .into_iter()
        .map(|comment| comment.body)
        .collect()
}

pub(crate) async fn assert_issue_comments_stay_empty(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let comments = issue_comment_bodies(forge, repo, number).await;
        assert!(
            comments.is_empty(),
            "unexpected issue comments: {comments:?}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) async fn assert_no_attention_mark(
    forge: &MemoryForge,
    repo: &RepositoryId,
    issue: ItemNumber,
) {
    assert!(
        !issue_labels(forge, repo, issue)
            .await
            .iter()
            .any(|label| label == "needs-human")
    );
    assert!(issue_comment_bodies(forge, repo, issue).await.is_empty());
}

pub(crate) async fn drop_issue_label(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    label: &str,
) {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                remove_labels: vec![label.to_string()],
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("label is dropped");
}

pub(crate) async fn drop_pull_request_label(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    label: &str,
) {
    let pull_request = forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request reload succeeds")
        .expect("pull request exists");
    forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                remove_labels: vec![label.to_string()],
                ..UpdatePullRequest::default()
            },
        )
        .await
        .expect("pull request label is dropped");
}

pub(crate) async fn assert_no_pull_requests(forge: &MemoryForge, repo: &RepositoryId) {
    let pulls = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await
        .expect("list pull requests succeeds");
    assert!(pulls.is_empty());
}

pub(crate) fn has_label(labels: &[String], expected: &str) -> bool {
    labels.iter().any(|label| label == expected)
}

pub(crate) async fn wait_for_review_apply(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    pull_request: ItemNumber,
    done: impl Fn(&[String], &[PullRequestReview]) -> bool,
) -> (Vec<String>, Vec<PullRequestReview>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let state = pull_request_labels_and_reviews(forge, repo, pull_request).await;
        if done(&state.0, &state.1) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for review verdict apply, saw labels {:?} reviews {:?}",
            state.0,
            state.1
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) async fn assert_pull_request_state_stays(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    expected_labels: Vec<String>,
    expected_reviews: usize,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let (labels, reviews) = pull_request_labels_and_reviews(forge, repo, number).await;
        assert_eq!(labels, expected_labels);
        assert_eq!(reviews.len(), expected_reviews);
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) async fn wait_for_pull_request_count(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) -> Vec<PullRequest> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let pulls = forge
            .list_pull_requests(repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        if pulls.len() == expected {
            return pulls;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} pull request(s), saw {}",
            pulls.len()
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) async fn assert_pull_request_count_stays(
    cx: &temper_engine_io::Cx,
    forge: &MemoryForge,
    repo: &RepositoryId,
    expected: usize,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        let pulls = forge
            .list_pull_requests(repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        assert_eq!(pulls.len(), expected);
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(crate) async fn assign_review_job(
    handle: &skein::runtime::RuntimeHandle,
    forge: Arc<MemoryForge>,
    repo: &RepositoryId,
    pull_request: ItemNumber,
) -> (
    temper_engine_io::http::JsonClient,
    String,
    temper_protocol_worker::Assign,
) {
    let workflow = Arc::new(workflow());
    let compiled = workflow.compile();
    let applier = Arc::new(LeaseApplier::new(
        forge.clone(),
        policy(),
        "daemon-1",
        Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
        temper_engine::system_clock(),
    ));
    let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
    let url = spawn(handle, &daemon).await;
    let client = temper_engine_io::http::JsonClient::new();
    let role = RoleId::new("reviewer");

    assert_eq!(
        post(
            &client,
            &url,
            &register("worker-a", "reviewer", "acme/service")
        )
        .await
        .status,
        204
    );

    assert_eq!(
        daemon
            .enqueue_scanned_role_work(
                forge.as_ref(),
                repo,
                workflow.as_ref(),
                &compiled,
                ts("2026-05-29T00:00:00Z"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("review feed succeeds"),
        1
    );

    let assignment = poll_review_assignment(&client, &url, "worker-a", pull_request).await;
    let context: JobContext = serde_json::from_value(assignment.job_payload.clone())
        .expect("assignment payload is a JobContext");
    assert_eq!(context.role, "reviewer");
    assert_eq!(context.queue, "pr_needs_review");
    assert_eq!(context.artifact_kind, "implementation_pr");
    assert_eq!(context.action.as_deref(), Some("review_pr"));
    assert_eq!(
        context.checkout_capability.as_deref(),
        Some("pull_request_read_only")
    );
    assert_eq!(
        context.allowed_verdicts,
        vec!["approve", "changes", "escalate"]
    );

    (client, url, assignment)
}
