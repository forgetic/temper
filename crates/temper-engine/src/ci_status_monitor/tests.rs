use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreatePullRequest, CreateRepository,
    Forge, HintArtifactKind, HintTarget, PullRequest, RepositoryPath, UserId,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::RawWorkflowSpec;

use super::*;

const CI_WORKFLOW: &str = r#"
{
  "name": "ci-monitor",
  "roles": [{ "id": "engineer", "queues": ["failed"] }],
  "labels": [{ "id": "implementation" }, { "id": "watch" }],
  "artifact_kinds": [
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] }
  ],
  "queues": [
    {
      "id": "failed",
      "artifact": "implementation_pr",
      "labels": ["watch"],
      "condition": { "kind": "ci_failed" }
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

fn target(id: &str, owner: &str, name: &str) -> RepositoryTarget {
    RepositoryTarget::new(RepositoryId::new(id), RepositoryPath::new(owner, name))
}

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid timestamp")
}

fn observation(
    number: u64,
    head_sha: &str,
    state: CiState,
    completed_at: Option<&str>,
) -> CiStatusObservation {
    CiStatusObservation {
        pull_request_number: ItemNumber::new(number),
        head_sha: head_sha.to_string(),
        current_head_ci_present: true,
        state,
        completed_at: completed_at.map(timestamp),
    }
}

fn monitor() -> CiStatusMonitor {
    CiStatusMonitor::new(
        Duration::from_secs(300),
        Arc::new(|| timestamp("2026-07-21T10:00:00Z")),
    )
}

fn terminal_transition(transition: &CiStatusTransition) -> &CiTerminalTransition {
    let CiStatusTransition::Terminal(transition) = transition else {
        panic!("expected terminal transition, got {transition:?}");
    };
    transition
}

fn assert_exact_transition(
    transition: &CiStatusTransition,
    repository: &RepositoryTarget,
    number: u64,
    head_sha: &str,
    verdict: CiTerminalVerdict,
    completed_at: Option<&str>,
) {
    let transition = terminal_transition(transition);
    assert_eq!(
        transition.hint,
        ChangeHint::pull_request(
            repository.path.clone(),
            ItemNumber::new(number),
            ChangeKind::Ci,
        )
    );
    assert_eq!(
        transition.hint.target,
        HintTarget::Artifact {
            kind: HintArtifactKind::PullRequest,
            number: ItemNumber::new(number),
        }
    );
    assert_eq!(transition.head_sha, head_sha);
    assert_eq!(transition.verdict, verdict);
    assert_eq!(transition.completed_at, completed_at.map(timestamp));
}

#[test]
fn pending_to_failed_and_pending_to_passed_emit_typed_transitions() {
    let repository = target("repo-1", "acme", "service");
    let mut monitor = monitor();

    assert!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![
                    observation(9, "head-failed", CiState::Pending, None),
                    observation(4, "head-passed", CiState::Pending, None),
                ],
            )
            .is_empty()
    );
    let transitions = monitor.observe_repository_snapshot(
        &repository,
        vec![
            observation(
                9,
                "head-failed",
                CiState::Failed,
                Some("2026-07-21T10:02:00Z"),
            ),
            observation(
                4,
                "head-passed",
                CiState::Passed,
                Some("2026-07-21T10:01:00Z"),
            ),
        ],
    );

    assert_eq!(transitions.len(), 2);
    assert_exact_transition(
        &transitions[0],
        &repository,
        4,
        "head-passed",
        CiTerminalVerdict::Passed,
        Some("2026-07-21T10:01:00Z"),
    );
    assert_exact_transition(
        &transitions[1],
        &repository,
        9,
        "head-failed",
        CiTerminalVerdict::Failed,
        Some("2026-07-21T10:02:00Z"),
    );
}

#[test]
fn first_seen_terminal_emits_and_identical_terminal_is_suppressed() {
    let repository = target("repo-1", "acme", "service");
    let terminal = observation(7, "head-1", CiState::Failed, Some("2026-07-21T11:00:00Z"));
    let mut monitor = monitor();

    let first = monitor.observe_repository_snapshot(&repository, vec![terminal.clone()]);
    assert_eq!(first.len(), 1);
    assert_exact_transition(
        &first[0],
        &repository,
        7,
        "head-1",
        CiTerminalVerdict::Failed,
        Some("2026-07-21T11:00:00Z"),
    );
    assert!(
        monitor
            .observe_repository_snapshot(&repository, vec![terminal])
            .is_empty()
    );
}

#[test]
fn failed_rerun_can_move_through_pending_to_passed_on_same_head() {
    let repository = target("repo-1", "acme", "service");
    let mut monitor = monitor();

    assert!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(7, "head-1", CiState::Pending, None)],
            )
            .is_empty()
    );
    let failed = monitor.observe_repository_snapshot(
        &repository,
        vec![observation(7, "head-1", CiState::Failed, None)],
    );
    assert_eq!(
        terminal_transition(&failed[0]).verdict,
        CiTerminalVerdict::Failed
    );
    assert!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(7, "head-1", CiState::Pending, None)],
            )
            .is_empty()
    );
    let passed = monitor.observe_repository_snapshot(
        &repository,
        vec![observation(7, "head-1", CiState::Passed, None)],
    );
    assert_eq!(passed.len(), 1);
    assert_eq!(
        terminal_transition(&passed[0]).verdict,
        CiTerminalVerdict::Passed
    );

    // A direct terminal change is also a transition when a short rerun
    // starts and finishes entirely between snapshots.
    let failed_again = monitor.observe_repository_snapshot(
        &repository,
        vec![observation(7, "head-1", CiState::Failed, None)],
    );
    assert_eq!(failed_again.len(), 1);
    assert_eq!(
        terminal_transition(&failed_again[0]).verdict,
        CiTerminalVerdict::Failed
    );
}

#[test]
fn a_new_head_is_observed_independently_and_supersedes_the_old_head() {
    let repository = target("repo-1", "acme", "service");
    let mut monitor = monitor();

    assert_eq!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(7, "head-old", CiState::Passed, None)],
            )
            .len(),
        1
    );
    let new_head = monitor.observe_repository_snapshot(
        &repository,
        vec![observation(7, "head-new", CiState::Passed, None)],
    );
    assert_eq!(new_head.len(), 1);
    assert_eq!(terminal_transition(&new_head[0]).head_sha, "head-new");
    assert_eq!(monitor.observations.len(), 1);
    assert!(
        monitor
            .observations
            .keys()
            .all(|key| key.head_sha == "head-new")
    );
    assert!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(7, "head-new", CiState::Passed, None)],
            )
            .is_empty()
    );
}

#[test]
fn successful_snapshots_prune_absent_pull_requests_and_superseded_heads() {
    let repository = target("repo-1", "acme", "service");
    let other = target("repo-2", "acme", "other");
    let mut monitor = monitor();
    monitor.observe_repository_snapshot(
        &repository,
        vec![
            observation(1, "one-old", CiState::Failed, None),
            observation(2, "two", CiState::Pending, None),
        ],
    );
    monitor
        .observe_repository_snapshot(&other, vec![observation(3, "other", CiState::Passed, None)]);

    assert!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(1, "one-new", CiState::Pending, None)],
            )
            .is_empty()
    );
    assert_eq!(monitor.observations.len(), 2);
    assert!(monitor.observations.keys().any(|key| {
        key.repository == repository.id
            && key.pull_request == ItemNumber::new(1)
            && key.head_sha == "one-new"
    }));
    assert!(!monitor.observations.keys().any(|key| {
        key.repository == repository.id
            && (key.pull_request == ItemNumber::new(2) || key.head_sha == "one-old")
    }));
    assert!(
        monitor
            .observe_repository_snapshot(&repository, Vec::new())
            .is_empty()
    );
    assert!(
        monitor
            .observations
            .keys()
            .all(|key| key.repository != repository.id)
    );
    assert!(
        monitor
            .observations
            .keys()
            .any(|key| key.repository == other.id)
    );

    // If the same PR/head appears after being absent (for example after a
    // close and reopen), it is first-seen state again.
    assert_eq!(
        monitor
            .observe_repository_snapshot(
                &repository,
                vec![observation(1, "one-old", CiState::Failed, None)],
            )
            .len(),
        1
    );
}

fn workflow() -> ValidatedWorkflow {
    serde_json::from_str::<RawWorkflowSpec>(CI_WORKFLOW)
        .expect("workflow parses")
        .validate()
        .expect("workflow validates")
}

fn create_repository(forge: &MemoryForge, name: &str) -> RepositoryTarget {
    let repository = block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created");
    RepositoryTarget::new(
        repository.id,
        RepositoryPath::new(repository.owner, repository.name),
    )
}

fn create_pull_request(
    forge: &MemoryForge,
    repository: &RepositoryTarget,
    head_sha: &str,
) -> PullRequest {
    create_pull_request_with_labels(forge, repository, head_sha, &["implementation", "watch"])
}

fn create_pull_request_with_labels(
    forge: &MemoryForge,
    repository: &RepositoryTarget,
    head_sha: &str,
    labels: &[&str],
) -> PullRequest {
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        CreatePullRequest {
            title: "CI-gated pull request".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repository.id.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repository.id.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created");
    forge
        .set_pull_request_head(&pull_request.id, Some(head_sha.to_string()))
        .expect("head is set")
}

fn terminal_job(
    repository: &RepositoryTarget,
    pull_request: &PullRequest,
    head_sha: &str,
    conclusion: CiJobConclusion,
) -> CiJob {
    let completed_at = timestamp("2026-07-21T12:00:00Z");
    CiJob {
        id: CiJobId::new(format!("{}-{}", repository.id, head_sha)),
        repo_id: repository.id.clone(),
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: head_sha.into(),
        name: "test".into(),
        status: CiJobStatus::Completed,
        conclusion: Some(conclusion),
        url: None,
        created_at: timestamp("2026-07-21T11:59:00Z"),
        started_at: Some(timestamp("2026-07-21T11:59:10Z")),
        completed_at: Some(completed_at),
        updated_at: completed_at,
    }
}

mod cadence;
mod missing;

#[test]
fn failed_repository_read_preserves_state_and_does_not_stop_other_repositories() {
    let forge = MemoryForge::new();
    let first_repository = create_repository(&forge, "alpha");
    let second_repository = create_repository(&forge, "beta");
    let first_pr = create_pull_request(&forge, &first_repository, "head-alpha");
    let second_pr = create_pull_request(&forge, &second_repository, "head-beta");
    forge.seed_ci_jobs(
        &first_repository.id,
        vec![terminal_job(
            &first_repository,
            &first_pr,
            "head-alpha",
            CiJobConclusion::Failure,
        )],
    );
    forge.seed_ci_jobs(
        &second_repository.id,
        vec![terminal_job(
            &second_repository,
            &second_pr,
            "head-beta",
            CiJobConclusion::Failure,
        )],
    );
    let repositories =
        RepositorySet::new(vec![second_repository.clone(), first_repository.clone()]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let mut monitor = monitor();

    let initial = block_on(run_ci_status_monitor_tick(
        &mut monitor,
        &forge,
        &repositories,
        &workflow,
        &compiled,
    ));
    assert_eq!(initial.len(), 2);
    assert!(initial.iter().all(|transition| {
        terminal_transition(transition).verdict == CiTerminalVerdict::Failed
    }));

    forge.seed_ci_jobs(
        &second_repository.id,
        vec![terminal_job(
            &second_repository,
            &second_pr,
            "head-beta",
            CiJobConclusion::Success,
        )],
    );
    // RepositorySet orders alpha before beta. The one-shot failure is
    // consumed by alpha; beta must still be read and emit its rerun pass.
    forge.fail_next(FaultOp::ListPullRequests, "alpha temporarily unavailable");
    let during_failure = block_on(run_ci_status_monitor_tick(
        &mut monitor,
        &forge,
        &repositories,
        &workflow,
        &compiled,
    ));
    assert_eq!(during_failure.len(), 1);
    assert_exact_transition(
        &during_failure[0],
        &second_repository,
        second_pr.number.get(),
        "head-beta",
        CiTerminalVerdict::Passed,
        Some("2026-07-21T12:00:00Z"),
    );

    // Alpha's previously emitted failure survived the failed read, so
    // recovery suppresses it rather than producing a duplicate.
    assert!(
        block_on(run_ci_status_monitor_tick(
            &mut monitor,
            &forge,
            &repositories,
            &workflow,
            &compiled,
        ))
        .is_empty()
    );
}
