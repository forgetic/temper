//! Aggregate request-budget regressions for the checked-in 17-label workflow.

use std::sync::Arc;
use std::time::{Duration, Instant};

use temper_engine::{CoordinatedMechanical, MechanicalBackstopConfig, MechanicalTrigger};
use temper_forge_forgejo::{
    EngineHttpClient, ForgejoConfig, ForgejoForge,
    MAX_PERIODIC_TERMINAL_CANDIDATE_PROVIDER_REQUESTS,
};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, CommitFile, CreateBranch, CreateIssue,
    CreateRepository, Forge, ForgeContent, IssueState, ItemNumberNamespace, MergeMethod,
    MergePullRequest, PullRequest, RepositoryId, RepositoryPath, UpdateIssue, UpsertLabel,
};
use temper_runner::{
    MechanicalWorker, RepositorySet, RepositoryTarget, TerminalDiscoveryRead,
    TerminalDiscoveryState, Worker, scan_automated_queues, scan_roles_wake,
    scan_roles_wake_with_discovery,
};
use temper_testing::block_on;
use temper_testing::counting_forge::{CountedForgeOp, CountingForge};
use temper_testing::counting_http::CountingHttpClient;
use temper_workflow::{InMemoryJournal, LeasePolicy, RoleId};

fn configured_roles() -> Vec<RoleId> {
    ["architect", "engineer", "reviewer", "owner", "human"]
        .into_iter()
        .map(RoleId::new)
        .collect()
}

fn assert_terminal_queries_are_labelled(
    issues: &[temper_forge_model::IssueCandidateQuery],
    pulls: &[temper_forge_model::PullRequestCandidateQuery],
) {
    for labels in issues
        .iter()
        .filter(|query| query.lifecycle == CandidateLifecycle::Terminal)
        .map(|query| &query.labels)
        .chain(
            pulls
                .iter()
                .filter(|query| query.lifecycle == CandidateLifecycle::Terminal)
                .map(|query| &query.labels),
        )
    {
        assert!(
            matches!(labels, CandidateLabelSelection::AnyOf(labels) if !labels.is_empty()),
            "terminal discovery must always carry workflow-derived labels: {labels:?}"
        );
    }
}

fn create_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    title: &str,
    branch: &str,
    labels: &[&str],
) -> PullRequest {
    block_on(forge.create_pull_request(
        repo,
        temper_testing::pull_request_input(
            repo,
            title,
            "",
            branch,
            labels.iter().map(|label| (*label).into()).collect(),
        ),
    ))
    .expect("PR is created")
}

fn seed_implementation_pr_dependency(forge: &MemoryForge, repo: &RepositoryId) {
    // MemoryForge intentionally has independent issue/PR counters. Skip the
    // two colliding issue numbers before modelling Forgejo's shared namespace.
    create_pr(forge, repo, "dummy one", "dummy-1", &[]);
    create_pr(forge, repo, "dummy two", "dummy-2", &[]);
    let target = create_pr(
        forge,
        repo,
        "Earlier implementation",
        "earlier-implementation",
        &["implementation", "in-progress"],
    );
    let blocked = create_pr(
        forge,
        repo,
        "Dependent implementation",
        "dependent-implementation",
        &["implementation", "in-progress"],
    );
    block_on(forge.add_pull_request_dependency(&blocked.id, target.number))
        .expect("PR dependency link is created");
}

#[test]
fn reference_role_reconciliation_and_automation_budgets_ignore_label_and_role_count() {
    let workflow = temper_testing::workflow();
    assert_eq!(workflow.labels().len(), 17, "checked-in budget reference");
    let compiled = workflow.compile();
    let memory = MemoryForge::new();
    let repository = block_on(memory.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created");
    let forge = CountingForge::new(memory);
    let roles = configured_roles();

    block_on(scan_roles_wake(
        &forge,
        &repository.id,
        &workflow,
        &compiled,
        temper_testing::ts("2026-05-29T00:00:00Z"),
        &roles,
    ))
    .expect("broad role discovery succeeds");
    let role_issue_queries = forge.issue_candidate_queries();
    let role_pull_queries = forge.pull_request_candidate_queries();
    assert!(
        role_issue_queries
            .len()
            .saturating_add(role_pull_queries.len())
            <= 4,
        "all configured roles and 17 labels share four lifecycle buckets"
    );
    assert_terminal_queries_are_labelled(&role_issue_queries, &role_pull_queries);

    let issue_before = role_issue_queries.len();
    let pull_before = role_pull_queries.len();
    block_on(scan_automated_queues(
        &forge,
        &repository.id,
        &workflow,
        &compiled,
        temper_testing::ts("2026-05-29T00:00:01Z"),
    ))
    .expect("automated discovery succeeds");
    let automated_issues = &forge.issue_candidate_queries()[issue_before..];
    let automated_pulls = &forge.pull_request_candidate_queries()[pull_before..];
    assert_eq!(
        automated_issues.len().saturating_add(automated_pulls.len()),
        2,
        "reference automation adds only populated open issue/PR buckets"
    );
    assert!(
        automated_issues
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
            && automated_pulls
                .iter()
                .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );

    let issue_before = forge.issue_candidate_queries().len();
    let pull_before = forge.pull_request_candidate_queries().len();
    block_on(
        workflow
            .reconciler(&temper_workflow::DefaultRecoveryPolicy)
            .reconcile(
                &forge,
                &repository.id,
                &InMemoryJournal::new(),
                temper_testing::ts("2026-05-29T00:00:02Z"),
            ),
    )
    .expect("bounded reconciliation succeeds");
    let reconciliation_issues = &forge.issue_candidate_queries()[issue_before..];
    let reconciliation_pulls = &forge.pull_request_candidate_queries()[pull_before..];
    assert!(
        reconciliation_issues
            .len()
            .saturating_add(reconciliation_pulls.len())
            <= 4,
        "bounded reconciliation uses at most four lifecycle buckets"
    );
    assert_terminal_queries_are_labelled(reconciliation_issues, reconciliation_pulls);
}

#[test]
fn long_lived_mechanical_trigger_warm_pass_has_candidate_lists_only() {
    let workflow = Arc::new(temper_testing::workflow());
    assert_eq!(workflow.labels().len(), 17, "checked-in budget reference");
    let memory = MemoryForge::new();
    let repository = block_on(memory.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created");
    let dependency = block_on(memory.create_issue(
        &repository.id,
        CreateIssue {
            title: "Unresolved design dependency".into(),
            body: String::new(),
            labels: vec!["design".into(), "draft".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("dependency issue is created");
    let blocked = block_on(memory.create_issue(
        &repository.id,
        CreateIssue {
            title: "Blocked code".into(),
            body: String::new(),
            labels: vec!["code".into(), "blocked".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("blocked issue is created");
    block_on(memory.add_issue_dependency(&blocked.id, dependency.number))
        .expect("dependency link is created");

    // Exercise a dependency-gated implementation PR whose PR target is also
    // present in the current candidate pass.
    seed_implementation_pr_dependency(&memory, &repository.id);

    let forge = Arc::new(CountingForge::with_item_number_namespace(
        memory,
        ItemNumberNamespace::Shared,
    ));
    let path = RepositoryPath::new("acme", "service");
    let target = RepositoryTarget::new(repository.id, path.clone());
    let trigger = MechanicalTrigger::new(
        Arc::clone(&forge),
        workflow,
        MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![target]),
            cadence: Duration::from_secs(300),
            lease_policy: LeasePolicy::new(chrono::Duration::minutes(30)),
            pull_request_merge_observer: None,
        },
        Arc::new(|| temper_testing::ts("2026-05-29T00:00:00Z")),
    );

    block_on(trigger.run_coordinated_broad(path.clone()))
        .expect("cold coordinated mechanical pass succeeds");
    let candidate_lists_before = forge
        .count(CountedForgeOp::ListIssueCandidates)
        .saturating_add(forge.count(CountedForgeOp::ListPullRequestCandidates));
    let issue_exact_before = forge.exact_issue_reads().len();
    let pull_exact_before = forge.exact_pull_request_reads().len();
    assert_eq!(trigger.reconciliation_detail_cache().len(), 3);

    block_on(trigger.run_coordinated_broad(path))
        .expect("warm coordinated mechanical pass succeeds");

    let candidate_lists_after = forge
        .count(CountedForgeOp::ListIssueCandidates)
        .saturating_add(forge.count(CountedForgeOp::ListPullRequestCandidates));
    assert_eq!(
        candidate_lists_after.saturating_sub(candidate_lists_before),
        5,
        "warm reference pass re-reads three reconciliation and two automation buckets"
    );
    assert_eq!(
        forge.exact_issue_reads().len(),
        issue_exact_before,
        "warm pass has no per-issue exact read"
    );
    assert_eq!(
        forge.exact_pull_request_reads().len(),
        pull_exact_before,
        "warm pass has no per-PR exact read"
    );
    assert_eq!(
        forge.read_shape().exact_full_reads,
        3,
        "only the cold pass requests dependency-enriched detail"
    );
}

#[path = "idle_request_budgets/history_independence.rs"]
mod history_independence;

#[test]
fn repeated_mechanical_and_role_budgets_ignore_large_labelled_terminal_history() {
    history_independence::assert_repeated_mechanical_and_role_budgets_ignore_large_labelled_terminal_history();
}

#[test]
#[ignore = "boots cached local Forgejo; run the documented idle-scan benchmark command"]
fn local_forgejo_two_pass_idle_broad_benchmark() {
    history_independence::run_local_forgejo_two_pass_idle_broad_benchmark();
}
