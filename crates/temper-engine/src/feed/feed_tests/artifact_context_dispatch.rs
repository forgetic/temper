// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_forge::{CreateIssue, RepositoryPath};
use temper_workflow::ArtifactRef;

const PLAN_DELIVERY_FIXTURE: &str =
    include_str!("../../../../../scenarios/plan-centric-feature-branch/config/workflow.json");

#[test]
fn configured_service_populates_normal_wake_and_recovered_dispatches_identically() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "temper".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap()
            .id;
        let create = |title: &str, body: String, labels: &[&str]| CreateIssue {
            title: title.to_string(),
            body,
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::new(),
        };
        let feature = forge
            .create_issue(
                &repo,
                create(
                    "feature",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("feature")),
                        ..Default::default()
                    }),
                    &["feature"],
                ),
            )
            .await
            .unwrap();
        let plan = forge
            .create_issue(
                &repo,
                create(
                    "plan",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("plan")),
                        parents: vec![ArtifactRef::same_repo(feature.number)],
                        target_branch: Some("feature/1".into()),
                        ..Default::default()
                    }),
                    &["plan", "in-progress"],
                ),
            )
            .await
            .unwrap();
        let code = forge
            .create_issue(
                &repo,
                create(
                    "code",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        parents: vec![ArtifactRef::same_repo(plan.number)],
                        target_branch: Some("feature/1".into()),
                        ..Default::default()
                    }),
                    &["code", "ready"],
                ),
            )
            .await
            .unwrap();
        let raw: RawWorkflowSpec = serde_json::from_str(PLAN_DELIVERY_FIXTURE).unwrap();
        let workflow = Arc::new(raw.validate().unwrap());
        let compiled = Arc::new(workflow.compile());
        let catalog = crate::ConfiguredRepositoryCatalog::single(
            repo.clone(),
            RepositoryPath::new("ai", "temper"),
            "https://forge.example",
        );
        let forge_handle: Arc<dyn Forge> = forge.clone();
        let service = Arc::new(crate::ArtifactContextService::new(
            forge_handle,
            workflow.clone(),
            catalog,
            crate::ArtifactContextPolicy::default(),
        ));
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp(10, 0).unwrap();
        let role = RoleId::new("engineer");

        let normal =
            Daemon::new(Arc::new(handle.clone())).with_artifact_context_service(service.clone());
        assert_eq!(
            normal
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    compiled.as_ref(),
                    now,
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .unwrap(),
            1
        );
        let normal_job = normal.queued_jobs().await.pop().unwrap();

        let wake = Daemon::new(Arc::new(handle)).with_artifact_context_service(service.clone());
        assert_eq!(
            wake.enqueue_scanned_role_work(
                forge.as_ref(),
                &repo,
                workflow.as_ref(),
                compiled.as_ref(),
                now,
                &role,
                RoleFeedMode::Wake,
            )
            .await
            .unwrap(),
            1
        );
        let wake_job = wake.queued_jobs().await.pop().unwrap();

        let job_id = format!("ai/temper/issue-{}/engineer/code_ready", code.number.get());
        let coordination_key = format!("pr-for-code-{}", code.number.get());
        let assignment = DurableAssignment {
            job_id: Some(job_id),
            role: Some(role),
            queue: Some("code_ready".into()),
            action: Some("open_pr".into()),
            coordination_key: Some(coordination_key),
            ..Default::default()
        };
        forge
            .update_issue(
                &code.id,
                UpdateIssue {
                    body: Some(render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        parents: vec![ArtifactRef::same_repo(plan.number)],
                        target_branch: Some("feature/1".into()),
                        assignment: Some(assignment.clone()),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let recovered = recovered_job_from_assignment_with_artifact_context(
            forge.as_ref(),
            &repo,
            ArtifactSource::Issue {
                number: code.number,
            },
            &assignment,
            workflow.as_ref(),
            compiled.as_ref(),
            service.as_ref(),
        )
        .await
        .unwrap();

        let contexts = [normal_job, wake_job, recovered].map(|job| {
            serde_json::from_value::<JobContext>(job.job_payload).expect("valid enriched context")
        });
        for context in contexts {
            assert_eq!(context.artifact.as_ref().unwrap().title, "code");
            assert_eq!(context.workspace.as_ref().unwrap().repos.len(), 1);
            assert_eq!(
                context
                    .artifact_context
                    .as_ref()
                    .unwrap()
                    .snapshots
                    .iter()
                    .map(|snapshot| snapshot.title.as_str())
                    .collect::<Vec<_>>(),
                ["feature", "plan", "code"]
            );
        }
    })
}
