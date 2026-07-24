// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn normal_dispatch_composes_configured_guidance_for_architect_engineer_and_tester() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repository = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created");
        let mut spec: RawWorkflowSpec = serde_json::from_str(include_str!(
            "../../../../../scenarios/plan-centric-feature-branch/config/workflow.json"
        ))
        .expect("plan-centric workflow parses");
        for queue in &mut spec.queues {
            for action in &mut queue.actions {
                action.guidance = Some(format!("Queue guidance for {}.", action.role));
            }
        }
        let workflow = spec.validate().expect("plan-centric workflow validates");
        let compiled = workflow.compile();

        let cases = [
            (
                "architect",
                "feature_planning",
                "feature",
                "Own product feature shaping.",
                "For feature_planning, decide whether",
                "Use this from feature_planning and decompose_plan.",
                "Only read the work-item context",
            ),
            (
                "engineer",
                "code_ready",
                "code",
                "Claim ready code issues",
                "Use open_pr for ready code issues.",
                "Use this for open_pr on ready code issues",
                "Only touch the checked-out repository workspace.",
            ),
            (
                "tester",
                "plan_needs_validation",
                "plan",
                "Validate completed feature plans",
                "Use validate_plan only after reading",
                "Use this from validate_plan.",
                "Tie validation evidence to the current feature branch head",
            ),
        ];

        for (role, queue, kind, charter, prompt, tool, constraint) in cases {
            let item = WorkItem {
                queue: QueueId::new(queue),
                role: RoleId::new(role),
                target: ArtifactSource::Issue {
                    number: temper_forge::ItemNumber::new(1),
                },
                kind: ArtifactKindId::new(kind),
            };
            let job = job_from_work_item("ai/temper", &item);
            let mut context: JobContext =
                serde_json::from_value(job.job_payload).expect("thin JobContext parses");
            context.source_metadata.insert(
                "target_branch".to_string(),
                "agent/pr-for-feature-1".to_string(),
            );

            enrich_job_context_from_workflow(
                &item,
                &workflow,
                &compiled,
                &repository,
                &mut context,
            )
            .expect("normal action assignment succeeds");

            let guidance = context
                .structured_guidance
                .expect("configured guidance is assigned");
            let role_guidance = guidance.role_guidance.expect("role guidance is assigned");
            assert!(
                role_guidance.starts_with(charter),
                "{role}: {role_guidance}"
            );
            assert!(role_guidance.contains(prompt), "{role}: {role_guidance}");
            assert_eq!(
                guidance.action_guidance.as_deref(),
                Some(format!("Queue guidance for {role}.").as_str()),
                "{role}: {:?}",
                guidance.action_guidance
            );
            assert!(
                guidance
                    .tool_guidance
                    .as_deref()
                    .is_some_and(|guidance| guidance.contains(tool)),
                "{role}: {:?}",
                guidance.tool_guidance
            );
            assert!(
                guidance
                    .tool_constraints
                    .iter()
                    .any(|configured| configured.contains(constraint)),
                "{role}: {:?}",
                guidance.tool_constraints
            );
        }
    });
}

#[test]
fn enrich_ci_failed_pull_request_becomes_writable_head_fix_with_guidance() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let source_coordination_key = "pr-for-code-226";
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement #226".to_string(),
                    body: format!(
                        "Applied the change.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            correlation_key: Some(source_coordination_key.to_string()),
                            ..WorkflowMetadata::default()
                        })
                    ),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-226".to_string(),
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
            .set_pull_request_head(&pull_request.id, Some("abc123".to_string()))
            .expect("pull request head sha is set");

        // Seed a FAILED CI job on the PR so the feed reads it into guidance.
        let head_sha = pull_request.head_sha.clone().unwrap_or_default();
        forge.seed_ci_jobs(
            &repo,
            vec![
                temper_forge::CiJob {
                    id: temper_forge::CiJobId::new("ci-validate-1"),
                    repo_id: repo.clone(),
                    pull_request_id: Some(pull_request.id.clone()),
                    commit_sha: head_sha,
                    name: "validate".to_string(),
                    status: temper_forge::CiJobStatus::Completed,
                    conclusion: Some(temper_forge::CiJobConclusion::Failure),
                    provider_conclusion: Some("failure".to_string()),
                    provider_reason: Some("process exited with code 1".to_string()),
                    run_id: Some("run-42".to_string()),
                    attempt: Some("2".to_string()),
                    url: Some("https://ci.example.test/jobs/validate".to_string()),
                    created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
                    started_at: None,
                    completed_at: None,
                    updated_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
                },
                temper_forge::CiJob {
                    id: temper_forge::CiJobId::new("ci-runner-lost-1"),
                    repo_id: repo.clone(),
                    pull_request_id: Some(pull_request.id.clone()),
                    commit_sha: pull_request.head_sha.clone().unwrap_or_default(),
                    name: "runner-lost-diagnostic".to_string(),
                    status: temper_forge::CiJobStatus::Completed,
                    conclusion: Some(temper_forge::CiJobConclusion::RunnerLost),
                    provider_conclusion: Some("failure".to_string()),
                    provider_reason: Some("runner disconnected".to_string()),
                    run_id: Some("run-43".to_string()),
                    attempt: Some("1".to_string()),
                    url: None,
                    created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
                    started_at: None,
                    completed_at: None,
                    updated_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
                },
            ],
        );

        // A `pr_ci_failed`-queue member for the implementation PR.
        let item = WorkItem {
            queue: QueueId::new("pr_ci_failed"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: ArtifactKindId::new("implementation_pr"),
        };
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds for ci-failed pull request"),
            EnrichOutcome::Enriched
        );

        let context: JobContext =
            serde_json::from_value(job.job_payload).expect("enriched JobContext parses");
        assert_eq!(context.action.as_deref(), Some("address_ci_failure"));
        // Writable checkout of the PR's REAL head branch (not a synthetic one).
        assert_eq!(
            context.checkout_capability.as_deref(),
            Some("pull_request_writable")
        );
        let freshness = context
            .pull_request_freshness
            .as_ref()
            .expect("PR-head freshness guard is present");
        assert_eq!(freshness.queue, "pr_ci_failed");
        assert_eq!(freshness.queue_condition.as_deref(), Some("ci_failed"));
        assert_eq!(freshness.pull_request_id, pull_request.id.as_str());
        assert_eq!(freshness.head_sha, pull_request.head_sha);
        let workspace = context.workspace.as_ref().expect("manifest present");
        assert_eq!(workspace.coordination_key, source_coordination_key);
        let primary = workspace.primary().expect("primary repo present");
        assert!(primary.is_writable());
        assert_eq!(
            primary.branch_hint.as_deref(),
            Some("agent/pr-for-code-226")
        );
        assert_eq!(primary.base_branch, "main");
        // Guidance surfaces the durable PR handoff plus fresh structured CI gate details.
        let guidance = context
            .structured_guidance
            .expect("ci-failure guidance present");
        assert!(
            guidance
                .tool_guidance
                .as_deref()
                .is_some_and(|configured| configured.contains("Use this for open_pr"))
        );
        assert!(
            guidance
                .tool_constraints
                .iter()
                .any(|constraint| constraint.contains("bookkeeping-only diffs"))
        );
        let role_guidance = guidance
            .role_guidance
            .expect("configured role guidance present");
        assert!(
            role_guidance.contains("Claim ready code issues"),
            "guidance: {role_guidance}"
        );
        let guidance = guidance
            .action_guidance
            .expect("generated CI guidance present");
        assert!(
            guidance.contains("Current implementation PR handoff from Forge"),
            "guidance: {guidance}"
        );
        assert!(guidance.contains("Implement #226"), "guidance: {guidance}");
        assert!(
            guidance.contains("Applied the change."),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("head_branch: agent/pr-for-code-226"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("base_branch: main"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("head_sha: abc123"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("reason: ci_failed"),
            "guidance: {guidance}"
        );
        assert!(guidance.contains("name: validate"), "guidance: {guidance}");
        assert!(
            !guidance.contains("runner-lost-diagnostic"),
            "non-repairable jobs must not be presented as source failures: {guidance}"
        );
        assert!(
            guidance.contains("status: completed"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("conclusion: failure"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("provider_conclusion: failure"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("provider_reason: process exited with code 1"),
            "guidance: {guidance}"
        );
        assert!(guidance.contains("run_id: run-42"), "guidance: {guidance}");
        assert!(guidance.contains("attempt: 2"), "guidance: {guidance}");
        assert!(
            guidance.contains("commit_sha: abc123"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("url: https://ci.example.test/jobs/validate"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("updated current PR `title`"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("implementation-report `body`"),
            "guidance: {guidance}"
        );

        let runner_lost = forge
            .list_ci_jobs(&repo, temper_forge::CiJobQuery::default())
            .await
            .expect("CI jobs remain readable")
            .into_iter()
            .find(|job| job.conclusion == Some(temper_forge::CiJobConclusion::RunnerLost))
            .expect("runner-lost evidence exists");
        forge.seed_ci_jobs(&repo, vec![runner_lost]);
        let mut stale_job = job_from_work_item("ai/temper", &item);
        let error =
            enrich_work_item_job(&forge, &repo, &item, &mut stale_job, &workflow, &compiled)
                .await
                .expect_err("non-repairable terminal CI cannot receive writable repair guidance");
        assert!(
            error.to_string().contains(
                "refusing stale writable code-repair guidance without explicit ordinary failure evidence"
            ),
            "error: {error}"
        );
    })
}
