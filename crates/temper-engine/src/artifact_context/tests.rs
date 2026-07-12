// SPDX-License-Identifier: MPL-2.0

use temper_forge::{CreateIssue, CreateRepository, Forge, IssueState, RepositoryPath, UpdateIssue};
use temper_forge_memory::MemoryForge;
use temper_runner::RepositoryTarget;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, RawWorkflowSpec, WorkflowMetadata,
    render_metadata_block,
};

use super::*;

const WORKFLOW: &str = include_str!("../../../temper-workflow/fixtures/reference-delivery.json");

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

        assert_eq!(
            bundle
                .snapshots
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["epic", "design", "code"]
        );
        assert_eq!(
            bundle
                .relations
                .iter()
                .filter(|relation| relation.relation_type == ArtifactRelationType::Parent)
                .count(),
            2
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

        assert_eq!(bundle.snapshots.len(), 1);
        assert!(
            bundle
                .index
                .iter()
                .any(|entry| entry.artifact.number == referenced.number.get())
        );
        assert!(!serde_json::to_string(&bundle).unwrap().contains("SECRET"));
        assert!(
            !bundle
                .index
                .iter()
                .any(|entry| entry.artifact.number == 999)
        );
    });
}
