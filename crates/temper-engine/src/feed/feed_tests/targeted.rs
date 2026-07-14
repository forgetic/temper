// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::BTreeSet;

fn targeted_basic_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let workflow: RawWorkflowSpec =
        serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
    let workflow = workflow.validate().expect("workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

async fn targeted_repo(forge: &MemoryForge) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap()
        .id
}

async fn targeted_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            temper_forge::CreateIssue {
                title: "ready".into(),
                body: "needs implementation".into(),
                labels: vec!["code".into(), "ready".into()],
                assignees: Vec::new(),
            },
        )
        .await
        .unwrap()
        .number
}

#[test]
fn targeted_feed_returns_artifact_scoped_ids_without_pruning_other_pending_work() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let forge = MemoryForge::new();
        let repo = targeted_repo(&forge).await;
        let first = targeted_ready_issue(&forge, &repo).await;
        let second = targeted_ready_issue(&forge, &repo).await;
        let repository = forge
            .get_repository(&repo)
            .await
            .expect("repository lookup succeeds")
            .expect("repository exists");
        let (workflow, compiled) = targeted_basic_workflow();
        let role = RoleId::new("engineer");

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    &forge,
                    &repo,
                    &workflow,
                    &compiled,
                    chrono::DateTime::from_timestamp(1, 0).unwrap(),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("broad seed feed succeeds"),
            2
        );

        let result = daemon
            .enqueue_targeted_role_work(
                &forge,
                &repository,
                &workflow,
                &compiled,
                chrono::DateTime::from_timestamp(2, 0).unwrap(),
                temper_runner::ArtifactAddress::issue(first),
                std::slice::from_ref(&role),
            )
            .await
            .expect("targeted feed succeeds");

        assert_eq!(result.enqueued, 1);
        assert_eq!(
            result.current_job_ids[&role],
            BTreeSet::from([format!(
                "ai/temper/issue-{}/engineer/code_ready",
                first.get()
            )])
        );
        let queued = daemon.queued_jobs().await;
        let queued_ids = queued
            .iter()
            .map(|job| job.job_id.clone())
            .collect::<BTreeSet<_>>();
        assert!(queued_ids.contains(&format!(
            "ai/temper/issue-{}/engineer/code_ready",
            first.get()
        )));
        assert!(queued_ids.contains(&format!(
            "ai/temper/issue-{}/engineer/code_ready",
            second.get()
        )));
    });
}

#[test]
fn targeted_terminal_feed_returns_empty_ids_for_only_the_selected_artifact() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let forge = MemoryForge::new();
        let repo = targeted_repo(&forge).await;
        let number = targeted_ready_issue(&forge, &repo).await;
        let issue = forge
            .get_issue_by_number(&repo, number)
            .await
            .unwrap()
            .unwrap();
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .unwrap();
        let repository = forge.get_repository(&repo).await.unwrap().unwrap();
        let (workflow, compiled) = targeted_basic_workflow();
        let role = RoleId::new("engineer");

        let result = daemon
            .enqueue_targeted_role_work(
                &forge,
                &repository,
                &workflow,
                &compiled,
                chrono::DateTime::from_timestamp(2, 0).unwrap(),
                temper_runner::ArtifactAddress::issue(number),
                std::slice::from_ref(&role),
            )
            .await
            .expect("targeted terminal feed succeeds");

        assert_eq!(result.enqueued, 0);
        assert!(result.current_job_ids[&role].is_empty());
        assert!(daemon.queued_jobs().await.is_empty());
    });
}
