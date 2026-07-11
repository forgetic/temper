use super::*;
use temper_forge::{CreatePullRequestReview, RequestReviewers, ReviewDecision, User, UserId};

fn reference_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let workflow: RawWorkflowSpec = serde_json::from_str(REFERENCE_DELIVERY_FIXTURE)
        .expect("reference-delivery workflow parses");
    let workflow = workflow
        .validate()
        .expect("reference-delivery workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

async fn new_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created")
        .id
}

#[test]
fn reference_open_pr_assignment_carries_decline_verdicts() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = forge
            .create_issue(
                &repo,
                temper_forge::CreateIssue {
                    title: "ready".to_string(),
                    body: "needs implementation".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let item = WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue {
                number: issue.number,
            },
            kind: ArtifactKindId::new("code"),
        };
        let mut job = job_from_work_item("ai/temper", &item);
        let (workflow, compiled) = reference_workflow();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds"),
            EnrichOutcome::Enriched
        );

        let context: JobContext = serde_json::from_value(job.job_payload).expect("context parses");
        assert_eq!(context.action.as_deref(), Some("open_pr"));
        assert_eq!(context.checkout_capability.as_deref(), Some("writable"));
        assert_eq!(
            context.allowed_verdicts,
            vec!["needs_architect".to_string(), "needs_human".to_string()]
        );
    })
}

#[test]
fn reference_review_assignment_is_read_only_pr_with_declared_verdicts() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement ready issue".to_string(),
                    body: "Please review.".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-9".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string(), "needs-reviewer".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        let item = WorkItem {
            queue: QueueId::new("pr_needs_review"),
            role: RoleId::new("reviewer"),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: ArtifactKindId::new("implementation_pr"),
        };
        let mut job = job_from_work_item("ai/temper", &item);
        let (workflow, compiled) = reference_workflow();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds"),
            EnrichOutcome::Enriched
        );

        let context: JobContext = serde_json::from_value(job.job_payload).expect("context parses");
        assert_eq!(context.action.as_deref(), Some("review_pr"));
        assert_eq!(
            context.checkout_capability.as_deref(),
            Some("pull_request_read_only")
        );
        assert_eq!(
            context.allowed_verdicts,
            vec![
                "approve".to_string(),
                "changes".to_string(),
                "escalate".to_string(),
            ]
        );
    })
}

#[test]
fn reference_pr_head_fix_assignments_checkout_real_pr_head() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let (workflow, compiled) = reference_workflow();

        for (queue, action) in [
            ("pr_changes_requested", "address_review_changes"),
            ("pr_merge_conflict", "resolve_merge_conflict"),
        ] {
            let head = format!("agent/{action}-head");
            let pull_request = forge
                .create_pull_request(
                    &repo,
                    CreatePullRequest {
                        title: format!("{action} target"),
                        body: "Existing implementation PR.".to_string(),
                        source: BranchRef {
                            repository_id: repo.clone(),
                            branch: head.clone(),
                        },
                        target: BranchRef {
                            repository_id: repo.clone(),
                            branch: "main".to_string(),
                        },
                        labels: vec!["implementation".to_string()],
                        assignees: Vec::new(),
                    },
                )
                .await
                .expect("pull request is created");
            let item = WorkItem {
                queue: QueueId::new(queue),
                role: RoleId::new("engineer"),
                target: ArtifactSource::PullRequest {
                    number: pull_request.number,
                },
                kind: ArtifactKindId::new("implementation_pr"),
            };
            let mut job = job_from_work_item("ai/temper", &item);

            assert_eq!(
                enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                    .await
                    .expect("enrichment succeeds"),
                EnrichOutcome::Enriched
            );

            let context: JobContext =
                serde_json::from_value(job.job_payload).expect("context parses");
            assert_eq!(context.action.as_deref(), Some(action));
            assert_eq!(
                context.checkout_capability.as_deref(),
                Some("pull_request_writable")
            );
            assert!(context.allowed_verdicts.is_empty());
            let primary = context
                .workspace
                .as_ref()
                .expect("workspace present")
                .primary()
                .expect("primary present");
            assert!(primary.is_writable());
            assert_eq!(primary.branch_hint.as_deref(), Some(head.as_str()));
            let guidance = context.guidance.expect("guidance present");
            assert!(guidance.contains(action), "guidance: {guidance}");
            assert!(guidance.contains(queue), "guidance: {guidance}");
            if queue == "pr_merge_conflict" {
                assert!(
                    guidance.contains("matched_labels: merge-conflict"),
                    "guidance: {guidance}"
                );
                assert!(
                    guidance.contains("merge conflict with main"),
                    "guidance: {guidance}"
                );
                assert!(
                    guidance.contains("Rebase or merge main"),
                    "guidance: {guidance}"
                );
            }
        }
    })
}

#[test]
fn advanced_head_recovery_publishes_merge_conflict_transition_once() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let (workflow, compiled) = reference_workflow();
        let queue = QueueId::new("pr_merge_conflict");
        let role = RoleId::new("engineer");
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Conflicted implementation".to_string(),
                    body: "Repair this PR.".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/conflicted".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec![
                        "implementation".to_string(),
                        "landing".to_string(),
                        "merge-conflict".to_string(),
                    ],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        let pull_request = forge
            .set_pull_request_head(&pull_request.id, Some("assigned-head".to_string()))
            .expect("assignment head is set");
        let item = WorkItem {
            queue: queue.clone(),
            role: role.clone(),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: ArtifactKindId::new("implementation_pr"),
        };
        let job = job_from_work_item("ai/temper", &item);
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            assignment: Some(DurableAssignment {
                job_id: Some(job.job_id),
                role: Some(role.clone()),
                queue: Some(queue.as_str().to_string()),
                action: Some("resolve_merge_conflict".to_string()),
                worker_id: Some("worker-before-restart".to_string()),
                coordination_key: Some("pr-for-code-restart".to_string()),
                assignment_pr_head: Some("assigned-head".to_string()),
                ..DurableAssignment::default()
            }),
            ..WorkflowMetadata::default()
        };
        forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    body: Some(format!(
                        "Repair this PR.\n\n{}",
                        render_metadata_block(&metadata)
                    )),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("assignment is persisted");
        forge
            .set_pull_request_head(&pull_request.id, Some("pushed-before-restart".to_string()))
            .expect("worker push is visible");
        assert_eq!(
            enqueue_scanned_role_work(
                &daemon,
                &forge,
                &repo,
                &workflow,
                &compiled,
                chrono::DateTime::from_timestamp(1, 0).expect("timestamp is valid"),
                &role,
                RoleFeedMode::Normal,
            )
            .await
            .expect("restart feed recovery succeeds"),
            0,
            "the old merge-conflict action is not redispatched"
        );
        assert!(daemon.queued_jobs().await.is_empty());

        let recovered = forge
            .get_pull_request_by_number(&repo, pull_request.number)
            .await
            .expect("pull request lookup succeeds")
            .expect("pull request remains open");
        assert!(recovered.labels.contains(&"landing".to_string()));
        assert!(!recovered.labels.contains(&"merge-conflict".to_string()));
        let metadata = parse_metadata_block(&recovered.body)
            .expect("metadata parses")
            .expect("metadata remains");
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());
        assert_eq!(
            metadata.repaired_head.as_deref(),
            Some("pushed-before-restart")
        );
    })
}

#[test]
fn reference_review_changes_requested_assignment_includes_review_feedback() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let (workflow, compiled) = reference_workflow();
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement reviewed feature".to_string(),
                    body: "Current implementation report.".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-227".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        let pull_request = forge
            .set_pull_request_head(&pull_request.id, Some("review-head-sha".to_string()))
            .expect("pull request head sha is set");
        let reviewer = User {
            id: UserId::new("reviewer-1"),
            handle: "reviewer-one".to_string(),
            display_name: None,
            email: None,
        };
        forge
            .request_pull_request_reviewers(
                &pull_request.id,
                RequestReviewers {
                    reviewers: vec![reviewer.id.clone()],
                },
            )
            .await
            .expect("reviewer is requested");
        let review = forge
            .as_user(reviewer)
            .submit_pull_request_review(
                &pull_request.id,
                CreatePullRequestReview {
                    decision: ReviewDecision::ChangesRequested,
                    body: Some("Please cover the edge case.\nAdd a regression test.".to_string()),
                },
            )
            .await
            .expect("changes-requested review is submitted");

        let item = WorkItem {
            queue: QueueId::new("pr_changes_requested"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: ArtifactKindId::new("implementation_pr"),
        };
        let mut job = job_from_work_item("ai/temper", &item);

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds for review feedback pull request"),
            EnrichOutcome::Enriched
        );

        let context: JobContext =
            serde_json::from_value(job.job_payload).expect("enriched JobContext parses");
        assert_eq!(context.action.as_deref(), Some("address_review_changes"));
        assert_eq!(
            context.checkout_capability.as_deref(),
            Some("pull_request_writable")
        );
        let artifact = context.artifact.as_ref().expect("PR snapshot is present");
        assert_eq!(artifact.title, "Implement reviewed feature");
        assert_eq!(artifact.body, "Current implementation report.");
        let freshness = context
            .pull_request_freshness
            .as_ref()
            .expect("PR-head freshness guard is present");
        assert_eq!(
            freshness.queue_condition.as_deref(),
            Some("review_changes_requested")
        );

        let guidance = context.guidance.expect("review guidance present");
        assert!(
            guidance.contains("Current implementation PR handoff from Forge"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("Implement reviewed feature"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("Current implementation report."),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("reason: review_changes_requested"),
            "guidance: {guidance}"
        );
        assert!(guidance.contains("reviewer-1"), "guidance: {guidance}");
        assert!(
            guidance.contains(&review.submitted_at.to_rfc3339()),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("Please cover the edge case."),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("Add a regression test."),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("updated current PR `title`"),
            "guidance: {guidance}"
        );
    })
}

#[test]
fn ambiguous_implicit_workspace_action_fails_loudly() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let issue = forge
            .create_issue(
                &repo,
                temper_forge::CreateIssue {
                    title: "ambiguous".to_string(),
                    body: "two possible actions".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let spec: RawWorkflowSpec = serde_json::from_value(json!({
            "name": "ambiguous-actions",
            "roles": [{"id": "engineer", "queues": ["code_ready"]}],
            "labels": [{"id": "code"}, {"id": "ready"}],
            "artifact_kinds": [{
                "id": "code",
                "target": "issue",
                "identifying_labels": ["code"]
            }],
            "queues": [{
                "id": "code_ready",
                "artifact": "code",
                "labels": ["ready"]
            }],
            "transitions": [
                {
                    "id": "action_a",
                    "artifact": "code",
                    "roles": ["engineer"],
                    "outcomes": {"done_a": "route_a"}
                },
                {
                    "id": "action_b",
                    "artifact": "code",
                    "roles": ["engineer"],
                    "outcomes": {"done_b": "route_b"}
                },
                {"id": "route_a", "artifact": "code", "roles": ["engineer"]},
                {"id": "route_b", "artifact": "code", "roles": ["engineer"]}
            ]
        }))
        .expect("raw workflow parses");
        let workflow = spec.validate().expect("workflow validates");
        let compiled = workflow.compile();
        let item = WorkItem {
            queue: QueueId::new("code_ready"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::Issue {
                number: issue.number,
            },
            kind: ArtifactKindId::new("code"),
        };
        let mut job = job_from_work_item("ai/temper", &item);

        let error = enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
            .await
            .expect_err("ambiguous action should fail");
        assert!(
            error
                .to_string()
                .contains("ambiguous workspace-backed fallback actions"),
            "error: {error}"
        );
    })
}
