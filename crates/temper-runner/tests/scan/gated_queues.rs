use super::*;

#[test]
fn ci_gated_automated_queue_fetches_ci_and_matches() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landing"]);
    seed_ci(&forge, &repo, number, CiJobConclusion::Success);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_automated_queues(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        vec![AutomatedWorkItem {
            queue: QueueId::new("landing"),
            actor: RoleId::new("mechanical"),
            transition: temper_workflow::TransitionId::new("land_pr"),
            executor: None,
            outcomes: std::collections::BTreeMap::from([(
                temper_workflow::VerdictId::merge_conflict(),
                temper_workflow::TransitionId::new("route_merge_conflict"),
            )]),
            target: ArtifactSource::PullRequest { number },
            kind: ArtifactKindId::new("implementation_pr"),
        }]
    );
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 1);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
}

#[test]
fn merge_conflict_label_pauses_landing_automation_without_clearing_landing() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(
        &forge,
        &repo,
        &["implementation", "landing", "merge-conflict"],
    );
    seed_ci(&forge, &repo, number, CiJobConclusion::Success);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert!(
        block_on(scan_automated_queues(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds")
        .is_empty()
    );
    assert_eq!(
        counting.count(CountedForgeOp::ListCiJobs),
        0,
        "excluded merge-conflict label should make landing fail before CI reads"
    );
}

#[test]
fn merged_landing_pr_with_passing_ci_is_not_an_automated_work_item() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landing", "landed"]);
    seed_ci(&forge, &repo, number, CiJobConclusion::Success);
    merge_pr(&forge, &repo, number);
    let workflow = workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert!(
        block_on(scan_automated_queues(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds")
        .is_empty()
    );
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
}

#[test]
fn review_gated_queue_fetches_reviews_but_not_ci() {
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
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 1);
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
}

#[test]
fn dependency_gated_queue_fetches_dependency_state() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let dependency = create_issue(&forge, &repo, &["code"]);
    close_issue(&forge, &repo, dependency);
    let blocked = create_issue(&forge, &repo, &["code", "blocked"]);
    add_issue_dependency(&forge, &repo, blocked, dependency);
    let workflow = workflow_from_json(DEPENDENCY_QUEUE_FIXTURE);
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());

    assert_eq!(
        block_on(scan_role(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &RoleId::new("dependency_watcher"),
        ))
        .expect("scan succeeds"),
        vec![WorkItem {
            queue: QueueId::new("dependencies_clear"),
            role: RoleId::new("dependency_watcher"),
            target: ArtifactSource::Issue { number: blocked },
            kind: ArtifactKindId::new("code"),
        }]
    );
    assert!(counting.count(CountedForgeOp::GetIssueByNumber) >= 2);
    assert!(
        counting
            .issue_queries()
            .iter()
            .all(|query| query.details == ItemListDetails::summary())
    );
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
}
