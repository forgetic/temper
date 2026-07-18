//! Persistent reconciliation dependency-detail cache regressions.

mod support;

use chrono::Duration as ChronoDuration;
use std::time::Duration;
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, HintArtifactKind,
    IssueState, ItemListDetails, ItemNumber, ItemNumberNamespace, MergeMethod, MergePullRequest,
    RepositoryId, UpdateIssue, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{MechanicalWorker, Worker};
use temper_workflow::{
    Applier, ArtifactKindId, ArtifactRef, ArtifactSource, DefaultRecoveryPolicy, DurableAssignment,
    Executor, InMemoryJournal, Lease, LeaseManager, LeasePolicy, ReconcileFinding,
    ReconciliationDetailCache, ReconciliationDetailCachePolicy, RecoveryAction, RoleId,
    WorkflowMetadata, render_metadata_block,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: temper_workflow::RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("fixture validates")
}

fn ts(value: &str) -> chrono::DateTime<chrono::Utc> {
    value.parse().expect("timestamp parses")
}

fn repo(forge: &MemoryForge, name: &str) -> RepositoryId {
    temper_testing::block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

fn issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: impl Into<String>,
) -> ItemNumber {
    temper_testing::block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "work".into(),
            body: body.into(),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("issue is created")
    .number
}

fn add_dependency(
    forge: &MemoryForge,
    repo: &RepositoryId,
    source: ItemNumber,
    target: ItemNumber,
) {
    let source = temper_testing::block_on(forge.get_issue_by_number(repo, source))
        .expect("source read succeeds")
        .expect("source exists");
    temper_testing::block_on(forge.add_issue_dependency(&source.id, target))
        .expect("dependency is added");
}

fn close(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let target = temper_testing::block_on(forge.get_issue_by_number(repo, number))
        .expect("target read succeeds")
        .expect("target exists");
    temper_testing::block_on(forge.update_issue(
        &target.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("target closes");
}

fn blocked_pair(forge: &MemoryForge, repo: &RepositoryId) -> (ItemNumber, ItemNumber) {
    // `design` participates in candidate discovery but has no dependency-gated
    // recovery transition, so it is a pure lifecycle-state target summary.
    let target = issue(forge, repo, &["design", "draft"], "");
    let source = issue(forge, repo, &["code", "blocked"], "");
    add_dependency(forge, repo, source, target);
    (source, target)
}

fn full_issue_read_count(forge: &CountingForge<MemoryForge>) -> usize {
    forge
        .exact_issue_reads()
        .iter()
        .filter(|read| read.details == ItemListDetails::full())
        .count()
}

fn reconcile(
    forge: &CountingForge<MemoryForge>,
    repo: &RepositoryId,
    cache: &ReconciliationDetailCache,
    now: chrono::DateTime<chrono::Utc>,
) -> temper_workflow::ReconcileReport {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    temper_testing::block_on(workflow.reconciler(&policy).reconcile_with_detail_cache(
        forge,
        repo,
        &InMemoryJournal::new(),
        now,
        cache,
    ))
    .expect("reconciliation succeeds")
}

#[path = "reconciliation_detail_cache/cache_behavior.rs"]
mod cache_behavior;
#[path = "reconciliation_detail_cache/target_and_invalidation.rs"]
mod target_and_invalidation;
