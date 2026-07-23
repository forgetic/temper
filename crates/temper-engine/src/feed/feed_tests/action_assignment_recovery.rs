use super::action_assignment::{new_repo, reference_workflow};
use super::*;

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
        forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    add_labels: vec!["needs-human".to_string()],
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("pull request is parked");
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
            .expect("parked recovery pass succeeds"),
            0
        );
        let parked = forge
            .get_pull_request_by_number(&repo, pull_request.number)
            .await
            .expect("pull request lookup succeeds")
            .expect("pull request remains open");
        assert!(parked.labels.contains(&"landing".to_string()));
        assert!(parked.labels.contains(&"merge-conflict".to_string()));
        assert!(parked.labels.contains(&"needs-human".to_string()));
        assert!(
            parse_metadata_block(&parked.body)
                .expect("metadata parses")
                .expect("metadata remains")
                .assignment
                .is_some(),
            "parked advanced-head recovery must retain its durable assignment"
        );

        forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    remove_labels: vec!["needs-human".to_string()],
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("human attention is cleared");
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
