//! Cross-repository dependency aggregation tests.

mod support;

use chrono::Duration;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, IssueState, ItemNumber,
    MergeMethod, MergePullRequest, RepositoryId, UpdateIssue, UserId,
};
use temper_forge_filesystem::FilesystemForge;
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::{
    Applier, ArtifactKindId, ArtifactRef, ArtifactSource, DefaultRecoveryPolicy, InMemoryJournal,
    LeaseManager, LeasePolicy, ReconcileFinding, RecoveryAction, TransitionId, WorkflowEffect,
    WorkflowMetadata, render_metadata_block,
};

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

struct FsRoot {
    path: PathBuf,
}

impl FsRoot {
    fn new() -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "temper-workflow-dependency-aggregation-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for FsRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn memory_cross_repo_dependencies_unblock_only_after_every_child_lands() {
    let forge = MemoryForge::new();
    assert_cross_repo_dependencies_unblock_only_after_every_child_lands(&forge);
}

#[test]
fn filesystem_cross_repo_dependencies_unblock_only_after_every_child_lands() {
    let root = FsRoot::new();
    let forge = root.forge();
    assert_cross_repo_dependencies_unblock_only_after_every_child_lands(&forge);
}

#[test]
fn cross_repo_pull_request_dependency_lands_when_merged_and_reports_read_failures() {
    let forge = MemoryForge::new();
    let workflow = support::workflow();
    let parent_repo = create_repo(&forge, "parent");
    let child_repo = create_repo(&forge, "child");
    let pr = create_pull_request(&forge, &child_repo);
    let target = ArtifactRef::in_repo(child_repo.clone(), pr);
    let parent_body = dependency_body(vec![target.clone()]);
    let parent = create_issue(
        &forge,
        &parent_repo,
        "parent",
        &["code", "blocked"],
        &parent_body,
    );

    forge.fail_next(
        FaultOp::GetPullRequestByNumber,
        "child PR temporarily unreadable",
    );
    let executor = workflow.executor(&forge);
    let signals = support::block_on(
        executor.read_gate_signals(&parent_repo, ArtifactSource::Issue { number: parent }),
    )
    .expect("child target read failure is captured in gate signals");
    assert!(!signals.dependencies().is_landed(&target));
    assert_eq!(signals.dependencies().read_failures().len(), 1);

    merge_pull_request(&forge, &child_repo, pr);
    let signals = support::block_on(
        executor.read_gate_signals(&parent_repo, ArtifactSource::Issue { number: parent }),
    )
    .expect("merged child PR dependency is readable");
    assert!(signals.dependencies().is_landed(&target));
    assert!(signals.dependencies().read_failures().is_empty());
}

#[test]
fn same_number_open_issue_prevents_pr_collision_from_satisfying_dependency() {
    let forge = MemoryForge::new();
    let workflow = support::workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    let repo = create_repo(&forge, "repo");
    let dependency_issue = create_issue(&forge, &repo, "dependency", &["code", "ready"], "");
    let dependent_body = dependency_body(vec![ArtifactRef::same_repo(dependency_issue)]);
    let dependent = create_issue(
        &forge,
        &repo,
        "dependent",
        &["code", "blocked"],
        &dependent_body,
    );
    let colliding_pr = create_pull_request(&forge, &repo);
    assert_eq!(
        colliding_pr, dependency_issue,
        "reference backends expose separate issue/PR counters, creating this collision"
    );

    merge_pull_request(&forge, &repo, colliding_pr);
    let report = reconcile(&forge, &workflow, &policy, &repo, &journal);
    assert!(
        report.is_clean(),
        "a same-number merged PR must not satisfy a dependency while the issue target is open"
    );

    close_issue(&forge, &repo, dependency_issue);
    let report = reconcile(&forge, &workflow, &policy, &repo, &journal);
    assert_eq!(
        report.findings,
        vec![ReconcileFinding::DependenciesResolved {
            target: ArtifactSource::Issue { number: dependent },
            transition: TransitionId::new("mark_code_ready"),
        }]
    );
}

#[test]
fn transient_child_repo_read_failure_is_not_a_false_unblock() {
    let forge = MemoryForge::new();
    let workflow = support::workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    let parent_repo = create_repo(&forge, "parent");
    let child_repo = create_repo(&forge, "child");
    let child = create_issue(&forge, &child_repo, "child", &["code", "ready"], "");
    close_issue(&forge, &child_repo, child);
    let parent_body = dependency_body(vec![ArtifactRef::in_repo(child_repo, child)]);
    create_issue(
        &forge,
        &parent_repo,
        "parent",
        &["code", "blocked"],
        &parent_body,
    );

    forge.fail_next(
        FaultOp::GetIssueByNumber,
        "child repo temporarily unreadable",
    );
    let report = support::block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &parent_repo,
        &journal,
        support::ts("2026-05-29T00:00:00Z"),
    ))
    .expect("a child read failure does not crash reconciliation");
    assert!(
        report.is_clean(),
        "an unreadable child is treated as not landed, never as a false unblock"
    );

    let report = support::block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &parent_repo,
        &journal,
        support::ts("2026-05-29T00:00:01Z"),
    ))
    .expect("reconciliation retries the fresh child read on the next scan");
    assert_eq!(report.findings.len(), 1);
    assert!(matches!(
        report.findings.as_slice(),
        [ReconcileFinding::DependenciesResolved { .. }]
    ));
}

fn assert_cross_repo_dependencies_unblock_only_after_every_child_lands<F: Forge + ?Sized>(
    forge: &F,
) {
    let workflow = support::workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    let parent_repo = create_repo(forge, "parent");
    let child_a_repo = create_repo(forge, "child-a");
    let child_b_repo = create_repo(forge, "child-b");
    let child_a = create_issue(forge, &child_a_repo, "child A", &["code", "ready"], "");
    let child_b = create_issue(forge, &child_b_repo, "child B", &["code", "ready"], "");
    let parent_body = dependency_body(vec![
        ArtifactRef::in_repo(child_a_repo.clone(), child_a),
        ArtifactRef::in_repo(child_b_repo.clone(), child_b),
    ]);
    let parent = create_issue(
        forge,
        &parent_repo,
        "parent",
        &["code", "blocked"],
        &parent_body,
    );

    let quiet = reconcile(forge, &workflow, &policy, &parent_repo, &journal);
    assert!(quiet.is_clean(), "no child has landed yet");

    close_issue(forge, &child_a_repo, child_a);
    let partial = reconcile(forge, &workflow, &policy, &parent_repo, &journal);
    assert!(partial.is_clean(), "one landed child is not enough");

    close_issue(forge, &child_b_repo, child_b);
    let complete = reconcile(forge, &workflow, &policy, &parent_repo, &journal);
    assert_eq!(
        complete.findings,
        vec![ReconcileFinding::DependenciesResolved {
            target: ArtifactSource::Issue { number: parent },
            transition: TransitionId::new("mark_code_ready"),
        }]
    );
    assert_eq!(
        complete.actions,
        vec![RecoveryAction::Unblock {
            target: ArtifactSource::Issue { number: parent },
            effects: vec![
                WorkflowEffect::RemoveLabel("blocked".into()),
                WorkflowEffect::AddLabel("ready".into()),
            ],
        }]
    );

    let executor = workflow.executor(forge);
    let leases = LeaseManager::new(forge, LeasePolicy::new(Duration::minutes(30)));
    let applier = Applier::new(&executor, &leases, &journal);
    let outcome = support::block_on(applier.apply_report(
        &parent_repo,
        &complete,
        support::ts("2026-05-29T00:00:02Z"),
    ))
    .expect("unblock applies");
    assert_eq!(
        outcome.applied.len(),
        1,
        "the full completion emits one unblock"
    );
    assert_eq!(
        issue_labels(forge, &parent_repo, parent),
        vec!["code".to_string(), "ready".to_string()]
    );

    let clean = reconcile(forge, &workflow, &policy, &parent_repo, &journal);
    assert!(
        clean.is_clean(),
        "after applying the unblock, the next scan is clean"
    );
}

fn reconcile<F: Forge + ?Sized>(
    forge: &F,
    workflow: &temper_workflow::ValidatedWorkflow,
    policy: &DefaultRecoveryPolicy,
    repo: &RepositoryId,
    journal: &InMemoryJournal,
) -> temper_workflow::ReconcileReport {
    support::block_on(workflow.reconciler(policy).reconcile_deep_audit(
        forge,
        repo,
        journal,
        support::ts("2026-05-29T00:00:00Z"),
    ))
    .expect("reconcile succeeds")
}

fn dependency_body(dependencies: Vec<ArtifactRef>) -> String {
    render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        dependencies,
        ..WorkflowMetadata::default()
    })
}

fn create_repo<F: Forge + ?Sized>(forge: &F, name: &str) -> RepositoryId {
    support::block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

fn create_issue<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    title: &str,
    labels: &[&str],
    body: &str,
) -> ItemNumber {
    support::block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: title.into(),
            body: body.into(),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("issue is created")
    .number
}

fn create_pull_request(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    support::block_on(forge.create_pull_request(
        repo,
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
            labels: vec!["implementation".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created")
    .number
}

fn merge_pull_request(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = support::block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    support::block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::MergeCommit,
            commit_title: None,
            commit_body: None,
        },
    ))
    .expect("pull request merges");
}

fn close_issue<F: Forge + ?Sized>(forge: &F, repo: &RepositoryId, number: ItemNumber) {
    let issue = support::block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    support::block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("issue closes");
}

fn issue_labels<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<String> {
    let mut labels = support::block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}
