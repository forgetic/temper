use super::*;

#[test]
fn overlapping_candidate_queries_deduplicate_artifacts() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready", "urgent", "bug"]);
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("branchy"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("code"),
        }]
    );
    // The overlapping open `any_of` branches still deduplicate to a single work
    // item. The total issue-list count also covers the bounded terminal-label
    // recovery queries now included on Normal role scans.
    assert_eq!(
        counting
            .issue_queries()
            .iter()
            .filter(|query| query.state == Some(IssueState::Open) && query.labels.is_empty())
            .count(),
        0,
        "no unlabelled open-all issue listing for this role"
    );
    assert!(counting.count(CountedForgeOp::ListIssues) >= 2);
}

#[test]
fn closed_unlabelled_history_does_not_change_scan_result_or_query_count() {
    #[derive(Debug, PartialEq)]
    struct ScanShape {
        items: Vec<WorkItem>,
        issue_calls: usize,
        pull_request_calls: usize,
        issue_queries: Vec<IssueQuery>,
        pull_request_queries: Vec<PullRequestQuery>,
        ci_calls: usize,
    }

    fn scan_with_closed_history(history: usize) -> ScanShape {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge);
        create_issue(&forge, &repo, &["code", "ready"]);
        for _ in 0..history {
            let issue = create_issue(&forge, &repo, &[]);
            close_issue(&forge, &repo, issue);
            let pull_request = create_pr(&forge, &repo, &[]);
            close_pr(&forge, &repo, pull_request);
        }
        let workflow = workflow_from_json(REFERENCE_FIXTURE);
        let compiled = workflow.compile();
        let counting = CountingForge::new(forge.clone());
        let items = block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("scan succeeds");
        ScanShape {
            items,
            issue_calls: counting.count(CountedForgeOp::ListIssues),
            pull_request_calls: counting.count(CountedForgeOp::ListPullRequests),
            issue_queries: counting.issue_queries(),
            pull_request_queries: counting.pull_request_queries(),
            ci_calls: counting.count(CountedForgeOp::ListCiJobs),
        }
    }

    let baseline = scan_with_closed_history(0);
    let with_history = scan_with_closed_history(200);

    assert_eq!(baseline, with_history);
    assert_eq!(with_history.ci_calls, 0);
    assert!(closed_issue_queries_have_labels(
        &with_history.issue_queries
    ));
    assert!(closed_pull_request_queries_have_labels(
        &with_history.pull_request_queries
    ));
}

#[test]
fn role_scan_does_not_request_closed_unlabelled_history() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let ready = create_issue(&forge, &repo, &["code", "ready"]);
    for _ in 0..25 {
        let issue = create_issue(&forge, &repo, &[]);
        close_issue(&forge, &repo, issue);
        let pull_request = create_pr(&forge, &repo, &[]);
        close_pr(&forge, &repo, pull_request);
    }
    let workflow = workflow_from_json(REFERENCE_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number: ready },
            kind: ArtifactKindId::new("code"),
        }]
    );
    assert!(closed_issue_queries_have_labels(&counting.issue_queries()));
    assert!(closed_pull_request_queries_have_labels(
        &counting.pull_request_queries()
    ));
    // Bounded terminal-label recovery queries are label-scoped, so unlabelled
    // closed history is never requested regardless of how much of it exists, and
    // the abundant unlabelled closed issues/PRs never pollute the scan result.
    assert!(
        !counting
            .issue_queries()
            .iter()
            .any(|query| query.state == Some(IssueState::Closed) && query.labels.is_empty())
    );
    assert!(!counting.pull_request_queries().iter().any(|query| {
        matches!(
            query.state,
            Some(PullRequestState::Closed | PullRequestState::Merged)
        ) && query.labels.is_empty()
    }));
}

#[test]
fn merged_pr_with_landed_queue_label_is_found_by_role_scans() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landed"]);
    merge_pr(&forge, &repo, number);
    let workflow = workflow_from_json(REFERENCE_FIXTURE);
    let compiled = workflow.compile();
    let expected = vec![WorkItem {
        queue: QueueId::new("landed_inbox"),
        role: RoleId::new("architect"),
        target: ArtifactSource::PullRequest { number },
        kind: ArtifactKindId::new("implementation_pr"),
    }];

    // Normal role scans now also surface the merged `landed` PR via the bounded
    // terminal-label recovery query, so poll-only fleets converge the terminal
    // `reconcile_landed` transition without needing a wake or audit.
    let normal_counting = CountingForge::new(forge.clone());
    assert_eq!(
        block_on(scan_role(
            &normal_counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("architect"),
        ))
        .expect("normal scan succeeds"),
        expected
    );
    assert!(has_pull_request_query(
        &normal_counting.pull_request_queries(),
        PullRequestState::Merged,
        &["landed"]
    ));

    let wake_counting = CountingForge::new(forge.clone());
    assert_eq!(
        block_on(scan_role_wake(
            &wake_counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("architect"),
        ))
        .expect("wake scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("landed_inbox"),
            role: RoleId::new("architect"),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert!(has_pull_request_query(
        &wake_counting.pull_request_queries(),
        PullRequestState::Merged,
        &["landed"]
    ));

    let audit_counting = CountingForge::new(forge.clone());
    assert_eq!(
        block_on(scan_role_audit(
            &audit_counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("architect"),
        ))
        .expect("audit scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("landed_inbox"),
            role: RoleId::new("architect"),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert!(has_pull_request_query(
        &audit_counting.pull_request_queries(),
        PullRequestState::Merged,
        &["landed"]
    ));
}

#[test]
fn open_all_fallback_queues_still_find_open_unlabelled_candidates() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"]);
    submit_review(&forge, &repo, number, ReviewDecision::ChangesRequested);
    let workflow = workflow_from_json(REVIEW_ONLY_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("review_watcher"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("review_changes"),
            role: RoleId::new("review_watcher"),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert!(counting.issue_queries().is_empty());
    assert_eq!(counting.count(CountedForgeOp::ListIssues), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequests), 1);
    assert_eq!(
        counting.pull_request_queries(),
        vec![PullRequestQuery {
            state: Some(PullRequestState::Open),
            labels: Vec::new(),
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        }]
    );
}
