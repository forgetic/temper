use super::*;

pub(super) fn broad_phase_measurements_include_provider_deltas_and_non_merge_has_no_attempt() {
    let memory = MemoryForge::new();
    let repo = create_repo(&memory);
    let repo_label = temper_log::strip_provider_scheme(repo.as_str()).to_string();
    create_ready_issue(&memory, &repo);
    let forge = CountingForge::new(memory);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &forge, &repo, &journal, lease_policy());

    let events = capture(|| {
        let wake = tracing::debug_span!("wake", wake.run_id = "acme/service:41");
        block_on(worker.tick(ts("2026-07-13T12:00:00Z")).instrument(wake))
            .expect("broad mechanical tick succeeds");
    });

    let phases = events_with_measurement(&events, "mechanical.phase");
    assert_eq!(phases.len(), 3, "one terminal event per broad phase");
    let names = phases
        .iter()
        .map(|event| event.text("mechanical.phase").expect("phase name"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from([
            "reconciliation".to_string(),
            "automated_scan".to_string(),
            "transition_application".to_string(),
        ])
    );
    for phase in phases {
        assert_phase_common(phase, &repo_label, "broad", "acme/service:41");
        assert_eq!(phase.text("outcome").as_deref(), Some("success"));
        assert_eq!(phase.bool("provider.requests_available"), Some(true));
        assert!(
            phase.u64("provider.request_total").is_some(),
            "provider request delta remains numeric: {phase:?}"
        );
    }

    let discoveries = events_with_measurement(&events, "candidate.discovery");
    assert_eq!(
        discoveries.len(),
        2,
        "broad reconciliation and automation each measure discovery"
    );
    for discovery in discoveries {
        assert_eq!(discovery.target, "temper::worker");
        assert_eq!(discovery.level, Level::DEBUG);
        assert_eq!(discovery.text("repo").as_deref(), Some(repo_label.as_str()));
        assert_eq!(
            discovery.text("candidate.consumer").as_deref(),
            Some("mechanical")
        );
        assert!(matches!(
            discovery.text("candidate.scope").as_deref(),
            Some("reconciliation" | "automation")
        ));
        assert!(discovery.u64("candidate.logical_bucket_count").is_some());
        assert!(discovery.u64("candidate.unique_count").is_some());
        assert!(discovery.u64("candidate.provider_request_total").is_some());
        assert_eq!(
            discovery.bool("candidate.provider_requests_available"),
            Some(true)
        );
        assert_eq!(discovery.text("outcome").as_deref(), Some("success"));
        assert!(discovery.u64("duration_ms").is_some());
        assert_eq!(
            discovery.span_text("wake.run_id").as_deref(),
            Some("acme/service:41")
        );
    }

    let reconciliation = events_with_measurement(&events, "mechanical.reconciliation");
    assert_eq!(reconciliation.len(), 1);
    let reconciliation = reconciliation[0];
    assert_eq!(
        reconciliation.text("mechanical.scope").as_deref(),
        Some("broad")
    );
    for field in [
        "detail_cache.hit_count",
        "detail_cache.miss_count",
        "detail_cache.forced_refresh_count",
        "detail_cache.invalidation_count",
        "detail_cache.eviction_count",
    ] {
        assert!(
            reconciliation.u64(field).is_some(),
            "{field} remains numeric: {reconciliation:?}"
        );
    }
    assert_eq!(
        reconciliation.span_text("wake.run_id").as_deref(),
        Some("acme/service:41")
    );
    assert!(
        events_with_measurement(&events, "mechanical.landing_attempt").is_empty(),
        "a direct non-merge automation is not a landing attempt"
    );
}

#[test]
fn broad_role_discovery_measurement_reports_shared_bucket_and_candidate_counts() {
    let memory = MemoryForge::new();
    let repo = create_repo(&memory);
    create_ready_issue(&memory, &repo);
    let forge = CountingForge::new(memory);
    let spec: RawWorkflowSpec =
        serde_json::from_str(REFERENCE_WORKFLOW).expect("reference workflow parses");
    let workflow = spec.validate().expect("reference workflow validates");
    let compiled = workflow.compile();
    let roles = ["architect", "engineer", "reviewer", "owner", "human"]
        .into_iter()
        .map(RoleId::new)
        .collect::<Vec<_>>();

    let events = capture(|| {
        let wake = tracing::debug_span!("wake", wake.run_id = "acme/service:role-42");
        block_on(
            scan_roles_wake(
                &forge,
                &repo,
                &workflow,
                &compiled,
                ts("2026-07-13T12:00:00Z"),
                &roles,
            )
            .instrument(wake),
        )
        .expect("broad role discovery succeeds");
    });

    let discoveries = events_with_measurement(&events, "candidate.discovery");
    assert_eq!(discoveries.len(), 1, "roles share one discovery plan");
    let discovery = discoveries[0];
    assert_eq!(
        discovery.text("candidate.consumer").as_deref(),
        Some("role")
    );
    assert_eq!(discovery.text("candidate.scope").as_deref(), Some("wake"));
    assert!(
        discovery
            .u64("candidate.logical_bucket_count")
            .is_some_and(|count| count <= 4)
    );
    assert!(
        discovery
            .u64("candidate.unique_count")
            .is_some_and(|count| count >= 1)
    );
    assert_eq!(
        discovery.span_text("wake.run_id").as_deref(),
        Some("acme/service:role-42")
    );
}
