// SPDX-License-Identifier: MPL-2.0

use super::*;

const CI_LANDING_WORKFLOW: &str = r#"
{
  "name": "ci-monitor-landing",
  "roles": [
    { "id": "engineer", "queues": ["failed"] },
    { "id": "mechanical" }
  ],
  "labels": [
    { "id": "implementation" },
    { "id": "landing" },
    { "id": "landed" }
  ],
  "artifact_kinds": [
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] }
  ],
  "queues": [
    {
      "id": "failed",
      "artifact": "implementation_pr",
      "condition": { "kind": "ci_failed" }
    },
    {
      "id": "landing",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "condition": { "kind": "ci_passed" },
      "automation": { "actor": "mechanical", "transition": "land_pr" }
    }
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "effects": [
        { "kind": "merge_pull_request" },
        { "kind": "remove_label", "label": "landing" },
        { "kind": "add_label", "label": "landed" }
      ]
    }
  ]
}
"#;

const REFERENCE_WORKFLOW: &str =
    include_str!("../../../../temper-workflow/fixtures/reference-delivery.json");

#[test]
fn terminal_red_cadence_uses_exact_coordinated_wake_and_enqueues_once() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repository = create_repository(forge.as_ref(), "service");
        let pull_request = create_pull_request(forge.as_ref(), &repository, "head-red");
        forge.seed_ci_jobs(
            &repository.id,
            vec![terminal_job(
                &repository,
                &pull_request,
                "head-red",
                CiJobConclusion::Failure,
            )],
        );
        let workflow = Arc::new(
            serde_json::from_str::<RawWorkflowSpec>(REFERENCE_WORKFLOW)
                .expect("reference workflow parses")
                .validate()
                .expect("reference workflow validates"),
        );
        let compiled = Arc::new(workflow.compile());
        let repositories = RepositorySet::new(vec![repository.clone()]);
        let role = temper_workflow::RoleId::new("engineer");
        let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle);
        let daemon = crate::Daemon::new(Arc::clone(&spawner)).with_wake_execution(
            Arc::clone(&forge),
            Arc::clone(&workflow),
            Arc::clone(&compiled),
            vec![crate::RoleFeedTarget {
                repo: repository.id.clone(),
                path: repository.path.clone(),
                role: role.clone(),
                mode: crate::RoleFeedMode::Wake,
            }],
            Arc::new(|| timestamp("2026-07-21T12:00:01Z")),
            None,
        );
        forge.fail_next(
            FaultOp::ListIssues,
            "CI cadence must not consume the issue-feed fault",
        );

        spawn_ci_status_monitor(
            &spawner,
            daemon.clone(),
            Arc::clone(&forge),
            repositories,
            workflow,
            compiled,
            Duration::from_millis(5),
            Duration::from_secs(300),
            Arc::new(|| timestamp("2026-07-21T12:00:01Z")),
        );
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(50)).await;

        let jobs = daemon.queued_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].role, "engineer");
        let context: temper_protocol_worker::JobContext =
            serde_json::from_value(jobs[0].job_payload.clone()).expect("job context parses");
        assert_eq!(context.queue, "pr_ci_failed");
        assert_eq!(context.action.as_deref(), Some("address_ci_failure"));
        assert!(
            forge
                .list_issues(&repository.id, temper_forge::IssueQuery::default())
                .await
                .is_err(),
            "the dedicated CI path performs no issue-feed scan"
        );
    });
}

#[test]
fn terminal_green_cadence_runs_targeted_mechanical_landing_without_fallback_scans() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repository = create_repository(forge.as_ref(), "green");
        let pull_request = create_pull_request_with_labels(
            forge.as_ref(),
            &repository,
            "head-green",
            &["implementation", "landing"],
        );
        forge.seed_ci_jobs(
            &repository.id,
            vec![terminal_job(
                &repository,
                &pull_request,
                "head-green",
                CiJobConclusion::Success,
            )],
        );
        let workflow = Arc::new(
            serde_json::from_str::<RawWorkflowSpec>(CI_LANDING_WORKFLOW)
                .expect("landing workflow parses")
                .validate()
                .expect("landing workflow validates"),
        );
        let compiled = Arc::new(workflow.compile());
        let repositories = RepositorySet::new(vec![repository.clone()]);
        let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle);
        let mechanical: Arc<dyn crate::CoordinatedMechanical> =
            Arc::new(crate::MechanicalTrigger::new(
                Arc::clone(&forge),
                Arc::clone(&workflow),
                crate::MechanicalBackstopConfig {
                    repositories: repositories.clone(),
                    cadence: Duration::from_secs(3_600),
                    lease_policy: temper_workflow::LeasePolicy::new(chrono::Duration::minutes(30)),
                    pull_request_merge_observer: None,
                },
                Arc::new(|| timestamp("2026-07-21T12:00:01Z")),
            ));
        let daemon = crate::Daemon::new(Arc::clone(&spawner)).with_wake_execution(
            Arc::clone(&forge),
            Arc::clone(&workflow),
            Arc::clone(&compiled),
            vec![crate::RoleFeedTarget {
                repo: repository.id.clone(),
                path: repository.path.clone(),
                role: temper_workflow::RoleId::new("engineer"),
                mode: crate::RoleFeedMode::Wake,
            }],
            Arc::new(|| timestamp("2026-07-21T12:00:01Z")),
            Some(mechanical),
        );

        spawn_ci_status_monitor(
            &spawner,
            daemon.clone(),
            Arc::clone(&forge),
            repositories,
            workflow,
            compiled,
            Duration::from_millis(5),
            Duration::from_secs(300),
            Arc::new(|| timestamp("2026-07-21T12:00:01Z")),
        );
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(50)).await;

        let landed = forge
            .get_pull_request_by_number(&repository.id, pull_request.number)
            .await
            .expect("pull request read succeeds")
            .expect("pull request still exists");
        assert_eq!(landed.state, temper_forge::PullRequestState::Merged);
        assert!(daemon.queued_jobs().await.is_empty());
    });
}
