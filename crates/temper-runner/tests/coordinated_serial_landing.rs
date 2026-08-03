//! Coordinated serial landing (ADR 0023, acyclic case).
//!
//! A coordinated pull-request set lands in dependency order: a dependent PR
//! does not merge until its prerequisite PR — possibly in another repo — has
//! merged, *even when the dependent's own CI is already green*. This is the
//! `dependency_gate` (`dependencies_resolved`) on `land_pr` gating on the
//! cross-repo dependency link the daemon writes into the PR metadata.
//!
//! The mechanical landing worker reads cross-repo dependency targets through
//! the shared Forge (`dependency_state::target_landed`), so a worker servicing
//! the dependent repo resolves the prerequisite PR in the other repo.

use chrono::{DateTime, Duration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreatePullRequest, CreateRepository,
    Forge, ItemNumber, PullRequestState, RepositoryId, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{MechanicalWorker, Progress, Worker};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, InMemoryJournal, LeasePolicy, RawWorkflowSpec, WorkflowMetadata,
    parse_metadata_block, render_metadata_block,
};

/// CI- and dependency-gated landing, no review (the basic-delivery shape, ADR
/// 0023): a PR lands once its CI passes AND every dependency link resolves.
const WORKFLOW: &str = r#"
{
  "name": "coordinated-serial-landing",
  "roles": [{ "id": "mechanical" }],
  "labels": [
    { "id": "implementation" },
    { "id": "landing" },
    { "id": "landed" }
  ],
  "artifact_kinds": [
    {
      "id": "implementation_pr",
      "target": "pull_request",
      "identifying_labels": ["implementation"]
    }
  ],
  "relations": [
    {
      "kind": "dependency",
      "source": "implementation_pr",
      "target": "implementation_pr"
    }
  ],
  "queues": [
    {
      "id": "landing",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "condition": { "kind": "ci_passed" },
      "automation": { "actor": "mechanical", "transition": "land_pr" }
    }
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "requires_gates": ["ci_gate", "dependency_gate"],
      "effects": [
        { "kind": "merge_pull_request" },
        { "kind": "remove_label", "label": "landing" },
        { "kind": "add_label", "label": "landed" }
      ]
    }
  ],
  "gates": [
    { "id": "ci_gate", "condition": { "kind": "ci_passed" } },
    { "id": "dependency_gate", "condition": { "kind": "dependencies_resolved" } }
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

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

fn create_repo(forge: &MemoryForge, owner: &str, name: &str) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: owner.into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

/// A PR already in the landing queue (`implementation` + `landing`), with the
/// given body (used to carry a cross-repo dependency metadata block).
fn create_landing_pr(forge: &MemoryForge, repo: &RepositoryId, body: String) -> ItemNumber {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "coordinated pr".into(),
            body,
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "agent/coord-for-code-7".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: vec!["implementation".into(), "landing".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

fn seed_passing_ci(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    forge.seed_ci_jobs(
        repo,
        vec![CiJob {
            id: CiJobId::new(format!("ci-{}-{}", repo.as_str(), number.get())),
            repo_id: repo.clone(),
            pull_request_id: Some(pull_request.id),
            commit_sha: format!("pr-{}-head", number.get()),
            name: "ci".into(),
            status: CiJobStatus::Completed,
            conclusion: Some(CiJobConclusion::Success),
            provider_conclusion: None,
            provider_reason: None,
            run_id: None,
            attempt: None,
            verified_failure: None,
            url: None,
            created_at: ts("2026-05-29T00:00:00Z"),
            started_at: Some(ts("2026-05-29T00:00:30Z")),
            completed_at: Some(ts("2026-05-29T00:01:00Z")),
            updated_at: ts("2026-05-29T00:01:00Z"),
        }],
    );
}

fn pr_state(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> PullRequestState {
    block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists")
        .state
}

#[test]
fn dependent_pr_lands_only_after_its_cross_repo_prerequisite_merges() {
    let forge = MemoryForge::new();
    let repo_prereq = create_repo(&forge, "acme", "lib"); // the prerequisite repo
    let repo_dependent = create_repo(&forge, "acme", "service"); // depends on lib

    // PR-A in the prerequisite repo has no dependencies.
    let pr_a = create_landing_pr(&forge, &repo_prereq, String::new());

    // PR-B in the dependent repo carries a cross-repo dependency link to PR-A
    // (exactly what the daemon writes for a coordinated set).
    let dependency_metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        dependencies: vec![ArtifactRef::in_repo(repo_prereq.clone(), pr_a)],
        ..WorkflowMetadata::default()
    };
    let pr_b = create_landing_pr(
        &forge,
        &repo_dependent,
        render_metadata_block(&dependency_metadata),
    );

    // Sanity: the dependency link is actually in PR-B's body and parses as a
    // cross-repo ArtifactRef to PR-A.
    let b_body = block_on(forge.get_pull_request_by_number(&repo_dependent, pr_b))
        .expect("lookup")
        .expect("pr b exists")
        .body;
    let parsed = parse_metadata_block(&b_body)
        .expect("metadata parses")
        .expect("metadata present");
    assert_eq!(
        parsed.dependencies,
        vec![ArtifactRef::in_repo(repo_prereq.clone(), pr_a)],
        "PR-B carries the cross-repo dependency link"
    );

    // BOTH PRs have green CI — so neither is held by the CI gate.
    seed_passing_ci(&forge, &repo_prereq, pr_a);
    seed_passing_ci(&forge, &repo_dependent, pr_b);

    let workflow = workflow();
    let journal_prereq = InMemoryJournal::new();
    let journal_dependent = InMemoryJournal::new();
    // One worker per repo, sharing the same Forge (so cross-repo dependency
    // targets resolve).
    let worker_prereq = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo_prereq,
        &journal_prereq,
        lease_policy(),
    );
    let worker_dependent = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo_dependent,
        &journal_dependent,
        lease_policy(),
    );

    // Phase 1 — the dependent's CI is green, but its prerequisite PR is still
    // open, so the dependency gate holds it closed: it MUST NOT land.
    assert_eq!(
        block_on(worker_dependent.tick(ts("2026-05-29T00:00:00Z")))
            .expect("dependent tick succeeds"),
        Progress::unchanged(),
        "the dependent PR is blocked by its unmerged prerequisite"
    );
    assert_eq!(
        pr_state(&forge, &repo_dependent, pr_b),
        PullRequestState::Open
    );

    // The prerequisite has no dependencies and green CI, so it lands.
    assert_eq!(
        block_on(worker_prereq.tick(ts("2026-05-29T00:00:10Z"))).expect("prereq tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(
        pr_state(&forge, &repo_prereq, pr_a),
        PullRequestState::Merged,
        "the prerequisite PR lands first"
    );

    // Phase 2 — with the prerequisite merged, the dependency gate opens and the
    // dependent PR finally lands. Serial order enforced.
    assert_eq!(
        block_on(worker_dependent.tick(ts("2026-05-29T00:00:20Z")))
            .expect("dependent tick succeeds"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    assert_eq!(
        pr_state(&forge, &repo_dependent, pr_b),
        PullRequestState::Merged,
        "the dependent PR lands only after its prerequisite merged"
    );
}
