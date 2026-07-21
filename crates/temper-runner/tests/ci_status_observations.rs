//! Regression coverage for the narrow CI-gated pull-request observation path.

mod support;

use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CandidateLabelSelection, CandidateLifecycle, CiJob, CiJobConclusion, CiJobId,
    CiJobStatus, CreateIssue, CreatePullRequest, CreateRepository, Forge, ItemNumber, PullRequest,
    RepositoryId, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{CiStatusObservation, read_ci_status_observations};
use temper_workflow::{
    ArtifactKindId, CiState, RawWorkflowSpec, WorkflowMetadata, render_metadata_block,
};

const CI_WORKFLOW: &str = r#"
{
  "name": "ci-observations",
  "roles": [
    { "id": "watcher", "queues": ["failed", "passed", "ordinary_issue"] }
  ],
  "labels": [
    { "id": "implementation" },
    { "id": "documentation" },
    { "id": "watch" },
    { "id": "landing" },
    { "id": "code" },
    { "id": "ready" }
  ],
  "artifact_kinds": [
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] },
    { "id": "documentation_pr", "target": "pull_request", "identifying_labels": ["documentation"] },
    { "id": "code", "target": "issue", "identifying_labels": ["code"] }
  ],
  "queues": [
    {
      "id": "failed",
      "artifact": "implementation_pr",
      "labels": ["watch"],
      "condition": { "kind": "ci_failed" }
    },
    {
      "id": "passed",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "condition": { "kind": "ci_passed" }
    },
    {
      "id": "ordinary_issue",
      "artifact": "code",
      "labels": ["ready"]
    }
  ]
}
"#;

const NO_CI_WORKFLOW: &str = r#"
{
  "name": "no-ci-observations",
  "roles": [{ "id": "reviewer", "queues": ["review"] }],
  "labels": [{ "id": "implementation" }],
  "artifact_kinds": [
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] }
  ],
  "queues": [
    {
      "id": "review",
      "artifact": "implementation_pr",
      "condition": { "kind": "review_changes_requested" }
    }
  ]
}
"#;

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
        Poll::Pending => panic!("in-memory forge futures should not park in tests"),
    }
}

fn workflow(json: &str) -> temper_workflow::ValidatedWorkflow {
    let raw: RawWorkflowSpec = serde_json::from_str(json).expect("workflow parses");
    raw.validate().expect("workflow validates")
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid timestamp")
}

fn new_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

fn create_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: String,
    head_sha: Option<&str>,
) -> PullRequest {
    let pull_request = block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "pull request".into(),
            body,
            source: BranchRef {
                repository_id: repo.clone(),
                branch: format!("feature-{}", labels.join("-")),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created");
    forge
        .set_pull_request_head(&pull_request.id, head_sha.map(str::to_owned))
        .expect("pull request head is set")
}

#[allow(clippy::too_many_arguments)]
fn ci_job(
    repo: &RepositoryId,
    pull_request: &PullRequest,
    id: &str,
    head_sha: &str,
    name: &str,
    status: CiJobStatus,
    conclusion: Option<CiJobConclusion>,
    created_at: &str,
    completed_at: Option<&str>,
) -> CiJob {
    CiJob {
        id: CiJobId::new(id),
        repo_id: repo.clone(),
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: head_sha.into(),
        name: name.into(),
        status,
        conclusion,
        url: None,
        created_at: ts(created_at),
        started_at: (status != CiJobStatus::Queued).then(|| ts(created_at)),
        completed_at: completed_at.map(ts),
        updated_at: completed_at.map_or_else(|| ts(created_at), ts),
    }
}

fn observations(
    forge: &impl Forge,
    repo: &RepositoryId,
    workflow: &temper_workflow::ValidatedWorkflow,
) -> Vec<CiStatusObservation> {
    block_on(read_ci_status_observations(
        forge,
        repo,
        workflow,
        &workflow.compile(),
    ))
    .expect("CI observations succeed")
}

#[test]
fn workflow_without_ci_pull_request_queues_performs_no_reads() {
    let inner = MemoryForge::new();
    let repo = new_repo(&inner);
    create_pr(
        &inner,
        &repo,
        &["implementation"],
        String::new(),
        Some("abcdef0123456789"),
    );
    let forge = CountingForge::new(inner);
    let workflow = workflow(NO_CI_WORKFLOW);

    assert!(observations(&forge, &repo, &workflow).is_empty());
    assert_eq!(forge.read_count(), 0);
    assert!(forge.issue_candidate_queries().is_empty());
    assert!(forge.pull_request_candidate_queries().is_empty());
    assert!(forge.ci_job_queries().is_empty());
}

#[test]
fn ci_discovery_uses_only_one_open_pull_request_bucket() {
    let inner = MemoryForge::new();
    let repo = new_repo(&inner);
    let pull_request = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("abcdef0123456789"),
    );
    block_on(inner.create_issue(
        &repo,
        CreateIssue {
            title: "ordinary issue".into(),
            body: String::new(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created");
    let forge = CountingForge::new(inner);
    let workflow = workflow(CI_WORKFLOW);

    assert_eq!(
        observations(&forge, &repo, &workflow),
        vec![CiStatusObservation {
            pull_request_number: pull_request.number,
            head_sha: "abcdef0123456789".into(),
            state: CiState::Pending,
            completed_at: None,
        }]
    );
    assert_eq!(forge.count(CountedForgeOp::ListIssueCandidates), 0);
    assert!(forge.issue_candidate_queries().is_empty());
    assert_eq!(forge.count(CountedForgeOp::ListPullRequestCandidates), 1);
    assert_eq!(forge.count(CountedForgeOp::ListPullRequests), 0);
    let queries = forge.pull_request_candidate_queries();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].lifecycle, CandidateLifecycle::Open);
    assert!(matches!(
        &queries[0].labels,
        CandidateLabelSelection::AnyOf(labels)
            if labels == &vec!["landing".to_string(), "watch".to_string()]
    ));
    assert_eq!(forge.count(CountedForgeOp::GetPullRequest), 1);
    assert_eq!(forge.count(CountedForgeOp::ListCiJobs), 1);
    assert_eq!(forge.write_count(), 0);
}

#[test]
fn candidates_are_classified_and_cheap_matched_before_ci_reads() {
    let inner = MemoryForge::new();
    let repo = new_repo(&inner);
    let relevant = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("abcdef0123456789"),
    );
    create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        render_metadata_block(&WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            staged: true,
            ..WorkflowMetadata::default()
        }),
        Some("bbbbbb0123456789"),
    );
    create_pr(
        &inner,
        &repo,
        &["documentation", "watch"],
        String::new(),
        Some("cccccc0123456789"),
    );
    create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("   "),
    );
    let forge = CountingForge::new(inner);
    let workflow = workflow(CI_WORKFLOW);

    let observed = observations(&forge, &repo, &workflow);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].pull_request_number, relevant.number);
    assert_eq!(forge.count(CountedForgeOp::GetPullRequest), 2);
    assert_eq!(
        forge.count(CountedForgeOp::ListCiJobs),
        1,
        "staged, irrelevant, and headless artifacts do not trigger CI reads"
    );
    assert_eq!(forge.write_count(), 0);
}

#[test]
fn observations_preserve_current_head_latest_attempt_semantics() {
    let inner = MemoryForge::new();
    let repo = new_repo(&inner);
    let no_jobs = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-no-jobs"),
    );
    let active = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-active"),
    );
    let mixed = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-mixed"),
    );
    let passed = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-passed"),
    );
    let failed = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-failed"),
    );
    let latest = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-latest"),
    );
    let stale = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-current"),
    );
    let rerun = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-rerun"),
    );

    let jobs = vec![
        ci_job(
            &repo,
            &active,
            "active-queued",
            "head-active",
            "build",
            CiJobStatus::Queued,
            None,
            "2026-05-29T00:00:01Z",
            None,
        ),
        ci_job(
            &repo,
            &active,
            "active-running",
            "head-active",
            "test",
            CiJobStatus::Running,
            None,
            "2026-05-29T00:00:02Z",
            None,
        ),
        ci_job(
            &repo,
            &mixed,
            "mixed-failed",
            "head-mixed",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
            "2026-05-29T00:00:03Z",
            Some("2026-05-29T00:01:03Z"),
        ),
        ci_job(
            &repo,
            &mixed,
            "mixed-running",
            "head-mixed",
            "test",
            CiJobStatus::Running,
            None,
            "2026-05-29T00:00:04Z",
            None,
        ),
        ci_job(
            &repo,
            &passed,
            "passed-build",
            "head-passed",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:05Z",
            Some("2026-05-29T00:01:05Z"),
        ),
        ci_job(
            &repo,
            &passed,
            "passed-test",
            "head-passed",
            "test",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:06Z",
            Some("2026-05-29T00:01:06Z"),
        ),
        ci_job(
            &repo,
            &failed,
            "failed-build",
            "head-failed",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
            "2026-05-29T00:00:07Z",
            Some("2026-05-29T00:01:07Z"),
        ),
        ci_job(
            &repo,
            &failed,
            "failed-test",
            "head-failed",
            "test",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:08Z",
            Some("2026-05-29T00:01:08Z"),
        ),
        // An older failed attempt has a later completion timestamp, but it is
        // not part of the latest-per-name aggregate.
        ci_job(
            &repo,
            &latest,
            "latest-old",
            "head-latest",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
            "2026-05-29T00:00:09Z",
            Some("2026-05-29T00:10:00Z"),
        ),
        ci_job(
            &repo,
            &latest,
            "latest-new",
            "head-latest",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:10Z",
            Some("2026-05-29T00:01:10Z"),
        ),
        ci_job(
            &repo,
            &latest,
            "latest-test",
            "head-latest",
            "test",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:11Z",
            Some("2026-05-29T00:01:11Z"),
        ),
        ci_job(
            &repo,
            &stale,
            "stale-success",
            "old-head",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:12Z",
            Some("2026-05-29T00:01:12Z"),
        ),
        ci_job(
            &repo,
            &stale,
            "current-queued",
            "head-current",
            "build",
            CiJobStatus::Queued,
            None,
            "2026-05-29T00:00:13Z",
            None,
        ),
        ci_job(
            &repo,
            &rerun,
            "rerun-old",
            "head-rerun",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
            "2026-05-29T00:00:14Z",
            Some("2026-05-29T00:01:14Z"),
        ),
        ci_job(
            &repo,
            &rerun,
            "rerun-new",
            "head-rerun",
            "build",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
            "2026-05-29T00:00:15Z",
            Some("2026-05-29T00:01:15Z"),
        ),
    ];
    inner.seed_ci_jobs(&repo, jobs);
    let forge = CountingForge::new(inner);
    let workflow = workflow(CI_WORKFLOW);

    let observed = observations(&forge, &repo, &workflow);
    assert_eq!(observed.len(), 8);
    let get = |number: ItemNumber| {
        observed
            .iter()
            .find(|observation| observation.pull_request_number == number)
            .expect("pull request was observed")
    };
    assert_eq!(get(no_jobs.number).state, CiState::Pending);
    assert_eq!(get(active.number).state, CiState::Pending);
    assert_eq!(get(mixed.number).state, CiState::Pending);
    assert_eq!(get(passed.number).state, CiState::Passed);
    assert_eq!(
        get(passed.number).completed_at,
        Some(ts("2026-05-29T00:01:06Z"))
    );
    assert_eq!(get(failed.number).state, CiState::Failed);
    assert_eq!(
        get(failed.number).completed_at,
        Some(ts("2026-05-29T00:01:08Z"))
    );
    assert_eq!(get(latest.number).state, CiState::Passed);
    assert_eq!(
        get(latest.number).completed_at,
        Some(ts("2026-05-29T00:01:11Z")),
        "completion comes only from the terminal latest-job set"
    );
    assert_eq!(get(stale.number).state, CiState::Pending);
    assert_eq!(get(rerun.number).state, CiState::Failed);
    assert!(
        observed
            .iter()
            .filter(|observation| observation.state == CiState::Pending)
            .all(|observation| observation.completed_at.is_none())
    );

    assert_eq!(forge.count(CountedForgeOp::ListCiJobs), 8);
    for query in forge.ci_job_queries() {
        assert!(query.pull_request_id.is_some());
        assert!(query.commit_sha.as_ref().is_some_and(|sha| !sha.is_empty()));
    }
    assert_eq!(forge.write_count(), 0);
}
