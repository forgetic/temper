use super::*;

#[test]
fn candidate_state_index_preserves_issue_first_collision_without_target_reads() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "collision");
    let target_issue = issue(&inner, &repo, &["design", "draft"], "");
    let colliding_pr = temper_testing::block_on(inner.create_pull_request(
        &repo,
        CreatePullRequest {
            title: "collision".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: Vec::new(),
            assignees: Vec::new(),
        },
    ))
    .expect("PR is created");
    assert_eq!(target_issue, colliding_pr.number);
    temper_testing::block_on(inner.merge_pull_request(
        &colliding_pr.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .expect("PR merges");
    let source = issue(&inner, &repo, &["code", "blocked"], "");
    add_dependency(&inner, &repo, source, target_issue);
    let forge = CountingForge::new(inner);

    let report = reconcile(
        &forge,
        &repo,
        &ReconciliationDetailCache::default(),
        ts("2026-05-29T00:00:00Z"),
    );
    assert!(
        report.is_clean(),
        "open issue wins over merged colliding PR"
    );
    assert_eq!(forge.count(CountedForgeOp::GetPullRequestByNumber), 0);
    assert_eq!(
        forge.exact_issue_reads(),
        vec![support::counting_forge::ExactIssueRead {
            by_number: true,
            details: ItemListDetails::full(),
        }],
        "only the dependency-bearing source needs full detail"
    );
}

#[test]
fn independent_namespace_probes_unlisted_issue_before_listed_pr() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "unlisted-collision");
    let target_issue = issue(&inner, &repo, &[], "");
    let colliding_pr = temper_testing::block_on(inner.create_pull_request(
        &repo,
        CreatePullRequest {
            title: "collision".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "collision".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: vec!["landed".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("PR is created");
    assert_eq!(target_issue, colliding_pr.number);
    temper_testing::block_on(inner.merge_pull_request(
        &colliding_pr.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .expect("PR merges");
    let source = issue(&inner, &repo, &["code", "blocked"], "");
    add_dependency(&inner, &repo, source, target_issue);
    let forge = CountingForge::new(inner);

    let report = reconcile(
        &forge,
        &repo,
        &ReconciliationDetailCache::default(),
        ts("2026-05-29T00:00:00Z"),
    );
    assert!(
        !report.findings.iter().any(|finding| matches!(
            finding,
            ReconcileFinding::DependenciesResolved {
                target: ArtifactSource::Issue { number },
                ..
            } if *number == source
        )),
        "open issue must win over listed merged PR: {report:?}"
    );
    assert_eq!(forge.exact_pull_request_reads().len(), 0);
    assert!(
        forge
            .exact_issue_reads()
            .iter()
            .any(|read| read.details == ItemListDetails::summary())
    );
}

#[test]
fn shared_namespace_listed_pr_target_avoids_collision_probe() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "listed-pr");
    let source = issue(&inner, &repo, &["code", "blocked"], "");
    let _dummy = temper_testing::block_on(inner.create_pull_request(
        &repo,
        CreatePullRequest {
            title: "dummy".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "dummy".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: Vec::new(),
            assignees: Vec::new(),
        },
    ))
    .expect("dummy PR is created");
    let target = temper_testing::block_on(inner.create_pull_request(
        &repo,
        CreatePullRequest {
            title: "target".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "target".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: vec!["landed".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("target PR is created");
    assert_eq!(target.number, ItemNumber::new(2));
    add_dependency(&inner, &repo, source, target.number);
    temper_testing::block_on(inner.merge_pull_request(
        &target.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .expect("target PR merges");
    let forge = CountingForge::with_item_number_namespace(inner, ItemNumberNamespace::Shared);

    let report = reconcile(
        &forge,
        &repo,
        &ReconciliationDetailCache::default(),
        ts("2026-05-29T00:00:00Z"),
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| matches!(finding, ReconcileFinding::DependenciesResolved { .. })),
        "unexpected report: {report:?}"
    );
    assert_eq!(
        forge.exact_pull_request_reads().len(),
        0,
        "present PR state must come from the fresh candidate index"
    );
    assert!(
        forge
            .exact_issue_reads()
            .iter()
            .all(|read| read.details == ItemListDetails::full()),
        "shared PR numbers must not trigger an issue summary collision probe"
    );
}

#[test]
fn unlisted_cross_repo_target_uses_summary_apis_without_dependency_detail() {
    let inner = MemoryForge::new();
    let parent_repo = repo(&inner, "parent");
    let child_repo = repo(&inner, "child");
    let target = temper_testing::block_on(inner.create_pull_request(
        &child_repo,
        CreatePullRequest {
            title: "dependency".into(),
            body: String::new(),
            source: BranchRef {
                repository_id: child_repo.clone(),
                branch: "feature".into(),
            },
            target: BranchRef {
                repository_id: child_repo.clone(),
                branch: "main".into(),
            },
            labels: Vec::new(),
            assignees: Vec::new(),
        },
    ))
    .expect("PR is created");
    temper_testing::block_on(inner.merge_pull_request(
        &target.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .expect("PR merges");
    let body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        dependencies: vec![ArtifactRef::in_repo(child_repo, target.number)],
        ..WorkflowMetadata::default()
    });
    let source = issue(&inner, &parent_repo, &["code", "blocked"], body);
    let forge = CountingForge::with_item_number_namespace(inner, ItemNumberNamespace::Shared);

    let report = reconcile(
        &forge,
        &parent_repo,
        &ReconciliationDetailCache::default(),
        ts("2026-05-29T00:00:00Z"),
    );
    assert_eq!(
        report.findings,
        vec![ReconcileFinding::DependenciesResolved {
            target: ArtifactSource::Issue { number: source },
            transition: temper_workflow::TransitionId::new("mark_code_ready"),
        }]
    );
    assert!(
        forge
            .exact_issue_reads()
            .iter()
            .any(|read| { read.by_number && read.details == ItemListDetails::summary() })
    );
    assert_eq!(
        forge.exact_pull_request_reads(),
        vec![support::counting_forge::ExactPullRequestRead {
            by_number: true,
            details: ItemListDetails::summary(),
        }],
        "target PR lifecycle read must omit its dependency links"
    );
}

#[test]
fn cache_hit_recovery_still_performs_executor_fresh_pre_mutation_read() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "recovery");
    let target = issue(&inner, &repo, &[], "");
    let source = issue(&inner, &repo, &["code", "blocked"], "");
    add_dependency(&inner, &repo, source, target);
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    let cache = ReconciliationDetailCache::default();

    let first = temper_testing::block_on(workflow.reconciler(&policy).reconcile_with_detail_cache(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:00:00Z"),
        &cache,
    ))
    .expect("cold reconciliation succeeds");
    assert!(first.is_clean());
    close(forge.inner(), &repo, target);

    let report =
        temper_testing::block_on(workflow.reconciler(&policy).reconcile_with_detail_cache(
            &forge,
            &repo,
            &journal,
            ts("2026-05-29T00:00:01Z"),
            &cache,
        ))
        .expect("warm reconciliation succeeds");
    assert_eq!(report.cache_stats.hits, 1);
    assert!(matches!(
        report.actions.as_slice(),
        [RecoveryAction::Unblock { .. }]
    ));
    let full_reads_before_apply = forge
        .exact_issue_reads()
        .iter()
        .filter(|read| read.details == ItemListDetails::full())
        .count();

    let executor = Executor::new(&workflow, &forge);
    let leases = LeaseManager::new(&forge, LeasePolicy::new(ChronoDuration::minutes(30)));
    temper_testing::block_on(Applier::new(&executor, &leases, &journal).apply_report(
        &repo,
        &report,
        ts("2026-05-29T00:00:01Z"),
    ))
    .expect("recovery applies");
    let full_reads_after_apply = forge
        .exact_issue_reads()
        .iter()
        .filter(|read| read.details == ItemListDetails::full())
        .count();
    assert!(
        full_reads_after_apply > full_reads_before_apply,
        "cached reconciliation snapshots never replace Executor's fresh read"
    );
}

#[test]
fn changed_repository_level_automation_conservatively_invalidates_cache() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "automation-invalidation");
    let source = issue(&inner, &repo, &[], "");
    let exact = temper_testing::block_on(inner.get_issue_by_number(&repo, source))
        .expect("source read succeeds")
        .expect("source exists");
    let cache = ReconciliationDetailCache::default();
    cache.store_issue(&repo, &exact, ts("2026-05-29T00:00:00Z"));
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(ChronoDuration::minutes(30)),
    )
    .with_reconciliation_detail_cache(cache.clone());

    let progress = temper_testing::block_on(worker.tick(ts("2026-05-29T00:00:01Z")))
        .expect("automation pass succeeds");
    assert!(progress.changed);
    assert!(
        cache.is_empty(),
        "repository-level changed progress invalidates conservatively"
    );
}

#[test]
fn targeted_dependency_hint_converges_immediately_and_invalidates_mutation() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "targeted-convergence");
    let (source, old_target) = blocked_pair(&inner, &repo);
    let new_target = issue(&inner, &repo, &["design", "draft"], "");
    close(&inner, &repo, new_target);
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let cache = ReconciliationDetailCache::default();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(ChronoDuration::minutes(30)),
    )
    .with_reconciliation_detail_cache(cache.clone());
    temper_testing::block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("cold pass succeeds");
    assert_eq!(cache.len(), 1);

    let current = temper_testing::block_on(forge.inner().get_issue_by_number(&repo, source))
        .expect("source read succeeds")
        .expect("source exists");
    temper_testing::block_on(
        forge
            .inner()
            .remove_issue_dependency(&current.id, old_target),
    )
    .expect("old dependency is removed");
    add_dependency(forge.inner(), &repo, source, new_target);

    let progress = temper_testing::block_on(worker.tick_artifact(
        ts("2026-05-29T00:00:01Z"),
        source,
        HintArtifactKind::Issue,
        temper_forge::ChangeKind::Dependency,
    ))
    .expect("targeted dependency pass succeeds");
    assert!(
        progress.changed,
        "dependency hint must converge in this pass"
    );
    assert!(
        cache.is_empty(),
        "known reconciliation mutation invalidates the affected entry"
    );
    let mut labels = temper_testing::block_on(forge.inner().get_issue_by_number(&repo, source))
        .expect("source read succeeds")
        .expect("source exists")
        .labels;
    labels.sort();
    assert_eq!(labels, vec!["code".to_string(), "ready".to_string()]);
}

#[test]
fn targeted_dependency_read_replaces_invalidated_entry_for_next_broad_pass() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "targeted");
    let (source, _) = blocked_pair(&inner, &repo);
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(ChronoDuration::minutes(30)),
    );

    temper_testing::block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("cold pass succeeds");
    assert_eq!(full_issue_read_count(&forge), 1);
    temper_testing::block_on(worker.tick_artifact(
        ts("2026-05-29T00:00:01Z"),
        source,
        HintArtifactKind::Issue,
        temper_forge::ChangeKind::Dependency,
    ))
    .expect("targeted dependency pass succeeds");
    assert_eq!(full_issue_read_count(&forge), 2);
    temper_testing::block_on(worker.tick(ts("2026-05-29T00:00:02Z"))).expect("warm pass succeeds");
    assert_eq!(
        full_issue_read_count(&forge),
        2,
        "successful targeted full read seeds the shared cache"
    );
}
