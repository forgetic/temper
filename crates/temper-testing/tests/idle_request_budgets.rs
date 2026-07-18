//! Aggregate request-budget regressions for the checked-in 17-label workflow.

use std::sync::Arc;
use std::time::{Duration, Instant};

use temper_engine::{CoordinatedMechanical, MechanicalBackstopConfig, MechanicalTrigger};
use temper_forge_forgejo::{EngineHttpClient, ForgejoConfig, ForgejoForge};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, CreateIssue, CreateRepository, Forge,
    RepositoryPath,
};
use temper_runner::{RepositorySet, RepositoryTarget, scan_automated_queues, scan_roles_wake};
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

    let forge = Arc::new(CountingForge::new(memory));
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
    assert_eq!(trigger.reconciliation_detail_cache().len(), 1);

    block_on(trigger.run_coordinated_broad(path))
        .expect("warm coordinated mechanical pass succeeds");

    let candidate_lists_after = forge
        .count(CountedForgeOp::ListIssueCandidates)
        .saturating_add(forge.count(CountedForgeOp::ListPullRequestCandidates));
    assert_eq!(
        candidate_lists_after.saturating_sub(candidate_lists_before),
        6,
        "warm reference pass re-reads four reconciliation and two automation buckets"
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
        1,
        "only the cold pass requests dependency-enriched detail"
    );
}

#[test]
#[ignore = "boots cached local Forgejo; run the documented idle-scan benchmark command"]
fn local_forgejo_two_pass_idle_broad_benchmark() {
    temper_engine_io::block_on(async move {
        let cached = skein::runtime::spawn_blocking(
            temper_testing::forgejo_server::start_cached_provisioned_server,
        )
        .await
        .expect("cached Forgejo fixture starts");
        let server = cached.server;
        let provisioned = cached.provisioned;
        let base_url = server.base_url().to_string();
        let setup = ForgejoForge::new(ForgejoConfig::new(&base_url, &provisioned.admin_token));
        let dependency = setup
            .create_issue(
                &provisioned.repository,
                CreateIssue {
                    title: "Idle benchmark unresolved dependency".into(),
                    body: String::new(),
                    labels: vec!["design".into(), "draft".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("benchmark dependency is created");
        let blocked = setup
            .create_issue(
                &provisioned.repository,
                CreateIssue {
                    title: "Idle benchmark blocked code".into(),
                    body: String::new(),
                    labels: vec!["code".into(), "blocked".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("benchmark source is created");
        setup
            .add_issue_dependency(&blocked.id, dependency.number)
            .await
            .expect("benchmark dependency link is created");

        let client = CountingHttpClient::new(EngineHttpClient::new(&base_url));
        let forge = Arc::new(ForgejoForge::with_client(
            ForgejoConfig::new(&base_url, &provisioned.admin_token),
            client.clone(),
        ));
        let path = RepositoryPath::new(&provisioned.owner, &provisioned.name);
        let target = RepositoryTarget::new(provisioned.repository.clone(), path.clone());
        let trigger = MechanicalTrigger::new(
            Arc::clone(&forge),
            Arc::new(temper_testing::workflow()),
            MechanicalBackstopConfig {
                repositories: RepositorySet::new(vec![target]),
                cadence: Duration::from_secs(300),
                lease_policy: LeasePolicy::new(chrono::Duration::minutes(30)),
                pull_request_merge_observer: None,
            },
            Arc::new(|| temper_testing::ts("2026-05-29T00:00:00Z")),
        );

        let cold_started = Instant::now();
        trigger
            .run_coordinated_broad(path.clone())
            .await
            .expect("cold broad pass succeeds");
        let cold_duration = cold_started.elapsed();
        let warm_start_index = client.request_count();
        let warm_started = Instant::now();
        trigger
            .run_coordinated_broad(path)
            .await
            .expect("warm broad pass succeeds");
        let warm_duration = warm_started.elapsed();

        println!("phase=broad.cold duration_ms={}", cold_duration.as_millis());
        println!("phase=broad.warm duration_ms={}", warm_duration.as_millis());
        for (method_path, count) in client.normalized_method_path_counts_since(warm_start_index) {
            println!("warm_requests count={count} {method_path}");
        }

        let warm_shape = client.forgejo_read_shape_since(warm_start_index);
        assert_eq!(
            warm_shape.candidate_list_requests, 6,
            "warm reference pass retains bounded summary discovery: {warm_shape:?}"
        );
        assert_eq!(
            warm_shape.exact_artifact_reads, 0,
            "warm pass must not reload per-artifact exact detail: {warm_shape:?}"
        );
        assert_eq!(
            warm_shape.dependency_requests, 0,
            "warm pass must not reload native dependencies: {warm_shape:?}"
        );
        assert_eq!(
            warm_shape.other_reads, 0,
            "idle warm pass must contain only candidate lists: {warm_shape:?}"
        );
    });
}
