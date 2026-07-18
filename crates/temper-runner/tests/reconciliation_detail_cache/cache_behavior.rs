use super::*;

#[test]
fn cache_cold_fill_warm_hit_fingerprint_refresh_forced_refresh_and_restart() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "service");
    let (source, _) = blocked_pair(&inner, &repo);
    let forge = CountingForge::new(inner);
    let cache = ReconciliationDetailCache::default();

    let cold = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:00:00Z"));
    assert_eq!(cold.cache_stats.misses, 1);
    assert_eq!(cold.cache_stats.hits, 0);
    assert_eq!(forge.exact_issue_reads().len(), 1);

    let warm = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:00:01Z"));
    assert_eq!(warm.cache_stats.hits, 1);
    assert_eq!(warm.cache_stats.misses, 0);
    assert_eq!(forge.exact_issue_reads().len(), 1);

    let current = temper_testing::block_on(forge.inner().get_issue_by_number(&repo, source))
        .expect("source read succeeds")
        .expect("source exists");
    temper_testing::block_on(forge.inner().update_issue(
        &current.id,
        UpdateIssue {
            body: Some("fresh heartbeat metadata".into()),
            ..UpdateIssue::default()
        },
    ))
    .expect("body updates");
    let changed = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:00:02Z"));
    assert_eq!(changed.cache_stats.misses, 1);
    assert_eq!(forge.exact_issue_reads().len(), 2);

    let forced = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:15:02Z"));
    assert_eq!(forced.cache_stats.forced_refreshes, 1);
    assert_eq!(forge.exact_issue_reads().len(), 3);

    let restarted = ReconciliationDetailCache::default();
    let restart_fill = reconcile(&forge, &repo, &restarted, ts("2026-05-29T00:15:03Z"));
    assert_eq!(restart_fill.cache_stats.misses, 1);
    assert_eq!(forge.exact_issue_reads().len(), 4);
}

#[test]
fn fresh_heartbeat_and_assignment_body_prevent_false_expiry_with_cache_enabled() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "heartbeat");
    let target = issue(&inner, &repo, &["design", "draft"], "");
    let assignment = |expires_at| DurableAssignment {
        job_id: Some("job-1".into()),
        role: Some(RoleId::new("engineer")),
        worker_id: Some("worker-1".into()),
        expires_at: Some(expires_at),
        ..DurableAssignment::default()
    };
    let lease = |heartbeat_at, expires_at| Lease {
        role: RoleId::new("engineer"),
        worker: "worker-1".into(),
        claimed_at: ts("2026-05-29T00:00:00Z"),
        heartbeat_at,
        expires_at,
    };
    let old_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        assignment: Some(assignment(ts("2026-05-29T00:10:00Z"))),
        lease: Some(lease(
            ts("2026-05-29T00:05:00Z"),
            ts("2026-05-29T00:10:00Z"),
        )),
        ..WorkflowMetadata::default()
    });
    let source = issue(&inner, &repo, &["code", "blocked"], old_body);
    add_dependency(&inner, &repo, source, target);
    let forge = CountingForge::new(inner);
    let cache = ReconciliationDetailCache::default();
    assert!(reconcile(&forge, &repo, &cache, ts("2026-05-29T00:06:00Z")).is_clean());

    let current = temper_testing::block_on(forge.inner().get_issue_by_number(&repo, source))
        .expect("source read succeeds")
        .expect("source exists");
    let fresh_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        assignment: Some(assignment(ts("2026-05-29T01:00:00Z"))),
        lease: Some(lease(
            ts("2026-05-29T00:19:00Z"),
            ts("2026-05-29T01:00:00Z"),
        )),
        ..WorkflowMetadata::default()
    });
    temper_testing::block_on(forge.inner().update_issue(
        &current.id,
        UpdateIssue {
            body: Some(fresh_body),
            ..UpdateIssue::default()
        },
    ))
    .expect("heartbeat updates");

    let report = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:20:00Z"));
    assert!(report.is_clean(), "fresh assignment and heartbeat must win");
    assert_eq!(
        report.cache_stats.misses, 1,
        "body fingerprint refreshes detail"
    );
    assert!(!report.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::ExpiredAssignment { .. } | ReconcileFinding::ExpiredLease { .. }
    )));
}

#[test]
fn forced_refresh_converges_when_dependency_hint_and_summary_advance_are_lost() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "dropped-hint");
    let (source, old_target) = blocked_pair(&inner, &repo);
    let new_target = issue(&inner, &repo, &["design", "draft"], "");
    close(&inner, &repo, new_target);
    let frozen_summary = temper_testing::block_on(inner.get_issue_by_number(&repo, source))
        .expect("source read succeeds")
        .expect("source exists");
    let forge = CountingForge::new(inner);
    forge.override_issue_candidate_summary(frozen_summary);
    let cache = ReconciliationDetailCache::default();

    let initial = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:00:00Z"));
    assert!(initial.is_clean());
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

    let before_bound = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:14:59Z"));
    assert!(
        before_bound.is_clean(),
        "stale cached link remains conservative"
    );
    assert_eq!(before_bound.cache_stats.hits, 1);

    let converged = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:15:00Z"));
    assert_eq!(converged.cache_stats.forced_refreshes, 1);
    assert!(matches!(
        converged.findings.as_slice(),
        [ReconcileFinding::DependenciesResolved { .. }]
    ));
}

#[test]
fn cache_enforces_deterministic_lru_and_unseen_age_eviction() {
    let inner = MemoryForge::new();
    let repo = repo(&inner, "bounded");
    blocked_pair(&inner, &repo);
    blocked_pair(&inner, &repo);
    let forge = CountingForge::new(inner);
    let cache = ReconciliationDetailCache::new(ReconciliationDetailCachePolicy::new(
        1,
        Duration::from_secs(15 * 60),
        Duration::from_secs(30 * 60),
    ));

    let first = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:00:00Z"));
    assert_eq!(first.cache_stats.misses, 2);
    assert_eq!(first.cache_stats.evictions, 1);
    assert_eq!(cache.len(), 1);

    let second = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:00:01Z"));
    assert_eq!(second.cache_stats.misses, 2);
    assert_eq!(second.cache_stats.evictions, 2);
    assert_eq!(cache.len(), 1);

    let aged = reconcile(&forge, &repo, &cache, ts("2026-05-29T00:31:00Z"));
    assert_eq!(aged.cache_stats.evictions, 2, "age eviction plus LRU");
    assert_eq!(aged.cache_stats.misses, 2);
    assert_eq!(cache.len(), 1);
}
