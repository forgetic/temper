use super::*;
use temper_forge::{HintArtifactKind, PullRequestUpdateState, UpdatePullRequest};
use temper_runner::{ArtifactAddress, targeted_role_work_items};

const TARGETED_WORKFLOW: &str = r#"
{
  "name": "targeted-role-scan",
  "roles": [
    { "id": "engineer", "queues": ["code_ready"] },
    { "id": "reviewer", "queues": ["review_changes"] },
    { "id": "backup_reviewer", "queues": ["review_changes"] },
    { "id": "ci_watcher", "queues": ["ci_failed"] },
    { "id": "unrelated", "queues": ["other_code"] }
  ],
  "labels": [
    { "id": "code" },
    { "id": "ready" },
    { "id": "other" },
    { "id": "implementation" }
  ],
  "artifact_kinds": [
    { "id": "code", "target": "issue", "identifying_labels": ["code"] },
    {
      "id": "implementation_pr",
      "target": "pull_request",
      "identifying_labels": ["implementation"]
    }
  ],
  "queues": [
    {
      "id": "code_ready",
      "artifact": "code",
      "labels": ["ready"]
    },
    {
      "id": "review_changes",
      "artifact": "implementation_pr",
      "condition": { "kind": "review_changes_requested" }
    },
    {
      "id": "ci_failed",
      "artifact": "implementation_pr",
      "condition": { "kind": "ci_failed" }
    },
    {
      "id": "other_code",
      "artifact": "code",
      "labels": ["other"]
    }
  ]
}
"#;

fn targeted_workflow() -> temper_workflow::ValidatedWorkflow {
    workflow_from_json(TARGETED_WORKFLOW)
}

fn targeted_scan<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    address: ArtifactAddress,
    roles: &[RoleId],
) -> Option<temper_runner::TargetedRoleScan> {
    let workflow = targeted_workflow();
    let compiled = workflow.compile();
    block_on(targeted_role_work_items(
        forge,
        repo,
        &workflow,
        &compiled,
        address,
        roles,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("targeted scan succeeds")
}

fn assert_no_broad_queries<F: Forge>(counting: &CountingForge<F>) {
    assert_eq!(counting.count(CountedForgeOp::ListIssues), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequests), 0);
}

#[test]
fn targeted_issue_reads_only_the_selected_namespace_and_filters_roles() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"]);
    let counting = CountingForge::new(forge.clone());

    let scan = targeted_scan(
        &counting,
        &repo,
        ArtifactAddress::issue(number),
        &[RoleId::new("engineer"), RoleId::new("reviewer")],
    )
    .expect("issue classifies");

    assert_eq!(
        scan.work_items,
        vec![WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("code"),
        }]
    );
    assert_eq!(counting.count(CountedForgeOp::GetIssueByNumber), 1);
    assert_eq!(counting.count(CountedForgeOp::GetPullRequestByNumber), 0);
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
    assert_no_broad_queries(&counting);
}

#[test]
fn targeted_closed_issue_and_queue_miss_still_use_one_exact_fetch_without_terminal_dispatch() {
    for close in [false, true] {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge);
        let labels = if close {
            vec!["code", "ready"]
        } else {
            vec!["code"]
        };
        let number = create_issue(&forge, &repo, &labels);
        if close {
            close_issue(&forge, &repo, number);
        }
        let counting = CountingForge::new(forge.clone());

        let scan = targeted_scan(
            &counting,
            &repo,
            ArtifactAddress::issue(number),
            &[RoleId::new("engineer")],
        )
        .expect("issue classifies");
        assert!(scan.work_items.is_empty());
        assert_eq!(counting.count(CountedForgeOp::GetIssueByNumber), 1);
        assert_eq!(counting.count(CountedForgeOp::GetPullRequestByNumber), 0);
        assert_no_broad_queries(&counting);
    }
}

#[test]
fn targeted_pr_unions_signal_needs_once_and_emits_subscribers_deterministically() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"]);
    submit_review(&forge, &repo, number, ReviewDecision::ChangesRequested);
    seed_ci(&forge, &repo, number, CiJobConclusion::Failure);
    let counting = CountingForge::new(forge.clone());

    let scan = targeted_scan(
        &counting,
        &repo,
        ArtifactAddress::pull_request(number),
        &[
            RoleId::new("ci_watcher"),
            RoleId::new("backup_reviewer"),
            RoleId::new("reviewer"),
        ],
    )
    .expect("pull request classifies");

    assert_eq!(
        scan.work_items,
        vec![
            WorkItem {
                queue: QueueId::new("review_changes"),
                role: RoleId::new("reviewer"),
                target: ArtifactSource::PullRequest { number },
                kind: ArtifactKindId::new("implementation_pr"),
            },
            WorkItem {
                queue: QueueId::new("review_changes"),
                role: RoleId::new("backup_reviewer"),
                target: ArtifactSource::PullRequest { number },
                kind: ArtifactKindId::new("implementation_pr"),
            },
            WorkItem {
                queue: QueueId::new("ci_failed"),
                role: RoleId::new("ci_watcher"),
                target: ArtifactSource::PullRequest { number },
                kind: ArtifactKindId::new("implementation_pr"),
            },
        ]
    );
    assert_eq!(counting.count(CountedForgeOp::GetPullRequestByNumber), 1);
    assert_eq!(counting.count(CountedForgeOp::GetIssueByNumber), 0);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 1);
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 1);
    assert_no_broad_queries(&counting);
}

#[test]
fn attention_pr_is_excluded_from_broad_and_targeted_role_wakes_before_signals() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "needs-human"]);
    submit_review(&forge, &repo, number, ReviewDecision::ChangesRequested);
    seed_ci(&forge, &repo, number, CiJobConclusion::Failure);
    let workflow = targeted_workflow();
    let compiled = workflow.compile();
    let counting = CountingForge::new(forge.clone());
    let roles = [RoleId::new("reviewer"), RoleId::new("ci_watcher")];

    assert!(
        block_on(scan_roles_wake(
            &counting,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
            &roles,
        ))
        .expect("broad wake succeeds")
        .is_empty()
    );
    let targeted = targeted_scan(
        &counting,
        &repo,
        ArtifactAddress::pull_request(number),
        &roles,
    )
    .expect("attention pull request still classifies");

    assert!(targeted.work_items.is_empty());
    assert_eq!(targeted.ci_status, None);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
}

#[test]
fn targeted_terminal_prs_skip_ci_without_issue_or_list_fallbacks() {
    for merged in [false, true] {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge);
        let number = create_pr(&forge, &repo, &["implementation"]);
        seed_ci(&forge, &repo, number, CiJobConclusion::Failure);
        let pull_request = block_on(forge.get_pull_request_by_number(&repo, number))
            .unwrap()
            .unwrap();
        if merged {
            block_on(forge.merge_pull_request(
                &pull_request.id,
                MergePullRequest {
                    method: MergeMethod::Squash,
                    commit_title: None,
                    commit_body: None,
                    delete_source_branch: false,
                },
            ))
            .unwrap();
        } else {
            block_on(forge.update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    state: Some(PullRequestUpdateState::Closed),
                    ..UpdatePullRequest::default()
                },
            ))
            .unwrap();
        }
        let counting = CountingForge::new(forge.clone());

        let scan = targeted_scan(
            &counting,
            &repo,
            ArtifactAddress::new(HintArtifactKind::PullRequest, number),
            &[RoleId::new("ci_watcher")],
        )
        .expect("terminal pull request still classifies");
        assert!(scan.work_items.is_empty());
        assert_eq!(counting.count(CountedForgeOp::GetPullRequestByNumber), 1);
        assert_eq!(counting.count(CountedForgeOp::GetIssueByNumber), 0);
        assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
        assert_no_broad_queries(&counting);
    }
}

#[test]
fn targeted_staged_pr_stops_before_signal_reads() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let pull_request = block_on(forge.create_pull_request(
        &repo,
        CreatePullRequest {
            title: "staged implementation".into(),
            body: render_metadata_block(&WorkflowMetadata {
                kind: Some(ArtifactKindId::new("implementation_pr")),
                staged: true,
                ..WorkflowMetadata::default()
            }),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: "staged".into(),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: vec!["implementation".into()],
            assignees: Vec::new(),
        },
    ))
    .unwrap();
    let counting = CountingForge::new(forge.clone());

    let scan = targeted_scan(
        &counting,
        &repo,
        ArtifactAddress::pull_request(pull_request.number),
        &[RoleId::new("reviewer"), RoleId::new("ci_watcher")],
    )
    .expect("staged pull request classifies");
    assert!(scan.work_items.is_empty());
    assert_eq!(counting.count(CountedForgeOp::GetPullRequestByNumber), 1);
    assert_eq!(counting.count(CountedForgeOp::ListPullRequestReviews), 0);
    assert_eq!(counting.count(CountedForgeOp::ListCiJobs), 0);
    assert_no_broad_queries(&counting);
}
