// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, IssueState, RepositoryPath,
    UpdateIssue,
};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::ArtifactRelationType;
use temper_runner::RepositoryTarget;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, RawWorkflowSpec, WorkflowMetadata,
    render_metadata_block,
};

use super::*;

const WORKFLOW: &str = include_str!("../../../temper-workflow/fixtures/reference-delivery.json");
const PLAN_WORKFLOW: &str =
    include_str!("../../../../scenarios/plan-centric-feature-branch/config/workflow.json");

mod projection;

fn issue(title: &str, body: String, labels: &[&str]) -> CreateIssue {
    CreateIssue {
        title: title.into(),
        body,
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        assignees: Vec::new(),
    }
}

#[test]
fn memory_forge_resolves_cross_repo_closed_lineage_root_first() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let primary_repo = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "temper".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let parent_repo = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "plans".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let epic = forge
            .create_issue(
                &parent_repo.id,
                issue(
                    "epic",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("epic")),
                        ..Default::default()
                    }),
                    &["epic"],
                ),
            )
            .await
            .unwrap();
        forge
            .update_issue(
                &epic.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let design = forge
            .create_issue(
                &parent_repo.id,
                issue(
                    "design",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("design")),
                        parents: vec![ArtifactRef::same_repo(epic.number)],
                        ..Default::default()
                    }),
                    &["design"],
                ),
            )
            .await
            .unwrap();
        let code = forge
            .create_issue(
                &primary_repo.id,
                issue(
                    "code",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        parents: vec![
                            ArtifactRef::in_repo(parent_repo.id.clone(), design.number),
                            ArtifactRef::in_repo(parent_repo.id.clone(), design.number),
                        ],
                        ..Default::default()
                    }),
                    &["code", "ready"],
                ),
            )
            .await
            .unwrap();
        let workflow: RawWorkflowSpec = serde_json::from_str(WORKFLOW).unwrap();
        let workflow = workflow.validate().unwrap();
        let catalog = ConfiguredRepositoryCatalog::new(
            [
                RepositoryTarget::new(primary_repo.id.clone(), RepositoryPath::new("ai", "temper")),
                RepositoryTarget::new(parent_repo.id.clone(), RepositoryPath::new("ai", "plans")),
            ],
            "https://forge.example",
        )
        .unwrap();

        let bundle = resolve_initial_artifact_context(
            &forge,
            &catalog,
            &workflow,
            &primary_repo.id,
            ArtifactSource::Issue {
                number: code.number,
            },
        )
        .await
        .unwrap();

        assert_eq!(bundle.primary.title, "code");
        assert_eq!(bundle.primary.workflow_kind.as_deref(), Some("code"));
        assert_eq!(
            bundle
                .lineage
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["epic", "design"]
        );
        assert!(
            bundle
                .diagnostics
                .iter()
                .any(|item| item.code == ArtifactContextDiagnosticCode::ClosedAncestor)
        );
    });
}

#[test]
fn memory_forge_keeps_reference_bodies_out_of_bundle() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repository = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "temper".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let referenced = forge
            .create_issue(
                &repository.id,
                issue("reference", "SECRET #999".into(), &["code", "ready"]),
            )
            .await
            .unwrap();
        let primary = forge
            .create_issue(
                &repository.id,
                issue(
                    "primary",
                    format!(
                        "See #{}\n{}",
                        referenced.number.get(),
                        render_metadata_block(&WorkflowMetadata {
                            kind: Some(ArtifactKindId::new("code")),
                            ..Default::default()
                        })
                    ),
                    &["code", "ready"],
                ),
            )
            .await
            .unwrap();
        let workflow: RawWorkflowSpec = serde_json::from_str(WORKFLOW).unwrap();
        let workflow = workflow.validate().unwrap();
        let catalog = ConfiguredRepositoryCatalog::single(
            repository.id.clone(),
            RepositoryPath::new("ai", "temper"),
            "https://forge.example",
        );

        let bundle = resolve_initial_artifact_context(
            &forge,
            &catalog,
            &workflow,
            &repository.id,
            ArtifactSource::Issue {
                number: primary.number,
            },
        )
        .await
        .unwrap();

        assert_eq!(bundle.primary.title, "primary");
        assert!(bundle.lineage.is_empty());
        assert!(
            bundle
                .optional_references
                .iter()
                .any(|entry| entry.artifact.number == referenced.number.get())
        );
        assert!(bundle.validation_scope.is_empty());
        assert!(!serde_json::to_string(&bundle).unwrap().contains("SECRET"));
        assert!(
            !bundle
                .optional_references
                .iter()
                .any(|entry| entry.artifact.number == 999)
        );
    });
}

#[test]
fn service_selects_mandatory_lineage_and_plan_validation_aggregates() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repository = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "temper".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let feature = forge
            .create_issue(
                &repository.id,
                issue(
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
                &repository.id,
                issue(
                    "plan",
                    render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("plan")),
                        parents: vec![ArtifactRef::same_repo(feature.number)],
                        target_branch: Some("feature/1".into()),
                        ..Default::default()
                    }),
                    &["plan", "needs-validation"],
                ),
            )
            .await
            .unwrap();
        let code = forge
            .create_issue(
                &repository.id,
                issue(
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
        let markdown_reference = forge
            .create_issue(
                &repository.id,
                issue("background", "optional context".into(), &["docs"]),
            )
            .await
            .unwrap();
        forge
            .update_issue(
                &plan.id,
                UpdateIssue {
                    body: Some(format!(
                        "See #{}\n{}",
                        markdown_reference.number.get(),
                        render_metadata_block(&WorkflowMetadata {
                            kind: Some(ArtifactKindId::new("plan")),
                            parents: vec![ArtifactRef::same_repo(feature.number)],
                            dependencies: vec![ArtifactRef::same_repo(code.number)],
                            target_branch: Some("feature/1".into()),
                            ..Default::default()
                        })
                    )),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let implementation = forge
            .create_pull_request(
                &repository.id,
                CreatePullRequest {
                    title: "implementation".into(),
                    body: render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("implementation_pr")),
                        parents: vec![ArtifactRef::same_repo(code.number)],
                        ..Default::default()
                    }),
                    source: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "agent/code".into(),
                    },
                    target: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "feature/1".into(),
                    },
                    labels: vec!["implementation".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();
        let landing = forge
            .create_pull_request(
                &repository.id,
                CreatePullRequest {
                    title: "landing".into(),
                    body: render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("feature_landing_pr")),
                        parents: vec![
                            ArtifactRef::same_repo(plan.number),
                            ArtifactRef::same_repo(feature.number),
                        ],
                        ..Default::default()
                    }),
                    source: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "feature/1".into(),
                    },
                    target: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "main".into(),
                    },
                    labels: vec!["feature-landing".into(), "merge-conflict".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();
        let workflow: RawWorkflowSpec = serde_json::from_str(PLAN_WORKFLOW).unwrap();
        let workflow = Arc::new(workflow.validate().unwrap());
        let catalog = ConfiguredRepositoryCatalog::single(
            repository.id.clone(),
            RepositoryPath::new("ai", "temper"),
            "https://forge.example",
        );
        let forge_handle: Arc<dyn Forge> = forge.clone();
        let service = ArtifactContextBundleService::new(
            forge_handle,
            workflow,
            catalog,
            ArtifactContextPolicy::default(),
        );

        let cases = [
            (
                ArtifactSource::Issue {
                    number: feature.number,
                },
                "plan_feature",
                vec![],
            ),
            (
                ArtifactSource::Issue {
                    number: plan.number,
                },
                "decompose_plan",
                vec!["feature"],
            ),
            (
                ArtifactSource::Issue {
                    number: code.number,
                },
                "open_pr",
                vec!["feature", "plan"],
            ),
            (
                ArtifactSource::PullRequest {
                    number: implementation.number,
                },
                "address_implementation_ci_failure",
                vec!["feature", "plan", "code"],
            ),
            (
                ArtifactSource::PullRequest {
                    number: landing.number,
                },
                "resolve_feature_landing_merge_conflict",
                vec!["feature", "plan"],
            ),
        ];
        for (source, action, expected_lineage) in cases {
            let bundle = service
                .resolve(&repository.id, source, action)
                .await
                .unwrap();
            let (expected_primary, expected_number, expected_kind) = match action {
                "plan_feature" => ("feature", feature.number.get(), "feature"),
                "decompose_plan" => ("plan", plan.number.get(), "plan"),
                "open_pr" => ("code", code.number.get(), "code"),
                "address_implementation_ci_failure" => (
                    "implementation",
                    implementation.number.get(),
                    "implementation_pr",
                ),
                "resolve_feature_landing_merge_conflict" => {
                    ("landing", landing.number.get(), "feature_landing_pr")
                }
                _ => unreachable!(),
            };
            assert_eq!(bundle.primary.title, expected_primary, "action {action}");
            assert_eq!(
                bundle.primary.artifact.number, expected_number,
                "action {action}"
            );
            assert_eq!(
                bundle.primary.workflow_kind.as_deref(),
                Some(expected_kind),
                "action {action}"
            );
            assert_eq!(
                bundle
                    .lineage
                    .iter()
                    .map(|snapshot| snapshot.title.as_str())
                    .collect::<Vec<_>>(),
                expected_lineage,
                "action {action}"
            );
        }

        let validation = service
            .resolve(
                &repository.id,
                ArtifactSource::Issue {
                    number: plan.number,
                },
                "validate_plan",
            )
            .await
            .unwrap();
        assert_eq!(validation.primary.title, "plan");
        assert_eq!(
            validation
                .lineage
                .iter()
                .map(|snapshot| snapshot.title.as_str())
                .collect::<Vec<_>>(),
            ["feature"]
        );
        let summaries = validation
            .validation_scope
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>();
        assert!(summaries.contains(&"code"));
        assert!(summaries.contains(&"implementation"));
        assert!(!summaries.contains(&"background"));
        assert_eq!(
            validation
                .optional_references
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["background"]
        );
        assert!(validation.validation_scope.iter().all(|summary| {
            !summary.labels.is_empty()
                && summary.source.number > 0
                && matches!(
                    summary.relation_type,
                    ArtifactRelationType::Dependency | ArtifactRelationType::Related
                )
        }));
    });
}
