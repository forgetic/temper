use super::*;

#[test]
fn untriaged_issue_yields_architect_triage_work() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["untriaged"]);

    assert_eq!(
        scan_repo(&forge, &repo),
        vec![WorkItem {
            queue: QueueId::new("design_triage"),
            role: RoleId::new("architect"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}

#[test]
fn staged_issue_is_never_returned_by_role_scans_despite_ready_labels() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "partially wired child".into(),
            body: render_metadata_block(&WorkflowMetadata {
                kind: Some(ArtifactKindId::new("code")),
                staged: true,
                ..WorkflowMetadata::default()
            }),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("staged issue is created");
    let workflow = workflow();
    let compiled = workflow.compile();

    assert!(scan_repo(&forge, &repo).is_empty());
    assert!(
        block_on(scan_role(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("normal role scan succeeds")
        .is_empty()
    );
    assert!(
        block_on(temper_runner::scan_role_wake(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("wake scan succeeds")
        .is_empty()
    );
    assert!(
        block_on(temper_runner::scan_role_audit(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("engineer"),
        ))
        .expect("audit scan succeeds")
        .is_empty()
    );
    assert_eq!(issue.number.get(), 1);
}

#[test]
fn ready_code_issue_yields_engineer_work() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"]);

    assert_eq!(
        scan_repo(&forge, &repo),
        vec![WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("code"),
        }]
    );
}

#[test]
fn failing_pr_ci_yields_engineer_work_but_passing_ci_does_not() {
    let workflow = workflow();
    let compiled = workflow.compile();
    let now = ts("2026-05-29T00:00:00Z");

    let failing_forge = MemoryForge::new();
    let failing_repo = new_repo(&failing_forge);
    let failing = create_pr(&failing_forge, &failing_repo, &["implementation"]);
    seed_ci(
        &failing_forge,
        &failing_repo,
        failing,
        CiJobConclusion::Failure,
    );

    assert_eq!(
        block_on(scan(
            &failing_forge,
            &failing_repo,
            &workflow,
            &compiled,
            now,
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("pr_ci_failed"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::PullRequest { number: failing },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );

    let passing_forge = MemoryForge::new();
    let passing_repo = new_repo(&passing_forge);
    let passing = create_pr(&passing_forge, &passing_repo, &["implementation"]);
    seed_ci(
        &passing_forge,
        &passing_repo,
        passing,
        CiJobConclusion::Success,
    );

    assert!(
        block_on(scan(
            &passing_forge,
            &passing_repo,
            &workflow,
            &compiled,
            now,
        ))
        .expect("scan succeeds")
        .is_empty()
    );
}

#[test]
fn empty_repo_yields_empty_worklist() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);

    assert!(scan_repo(&forge, &repo).is_empty());
}

#[test]
fn unclassified_artifacts_are_ignored() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    create_issue(&forge, &repo, &[]);

    assert!(scan_repo(&forge, &repo).is_empty());
}

#[test]
fn role_scan_returns_only_the_roles_subscribed_queues() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let untriaged = create_issue(&forge, &repo, &["untriaged"]);
    create_issue(&forge, &repo, &["code", "ready"]);
    let workflow = workflow();
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_role(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("architect"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("design_triage"),
            role: RoleId::new("architect"),
            target: ArtifactSource::Issue { number: untriaged },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}

#[test]
fn role_scan_without_ci_gated_queue_does_not_list_ci_jobs() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    create_pr(&forge, &repo, &["implementation"]);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    let items = block_on(scan_role(
        &counting,
        &repo,
        &workflow,
        &compiled,
        ts("2026-05-29T00:00:00Z"),
        &RoleId::new("architect"),
    ))
    .expect("scan succeeds");

    assert!(items.is_empty());
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
    assert!(
        counting
            .issue_queries()
            .iter()
            .all(|query| query.details == ItemListDetails::summary())
    );
    assert!(
        counting
            .pull_request_queries()
            .iter()
            .all(|query| query.details == ItemListDetails::summary())
    );
}
