use super::*;

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
        }
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
