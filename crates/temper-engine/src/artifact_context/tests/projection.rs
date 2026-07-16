// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_forge::{CreateRepository, Forge, RepositoryPath, UpdateIssue};
use temper_forge_memory::MemoryForge;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, CreateIssueIntentChild, CreateIssuesCompletion,
    CreateIssuesIntent, DurableAssignment, Lease, RawWorkflowSpec, RoleId, WorkflowMetadata,
    render_metadata_block,
};

use super::{PLAN_WORKFLOW, issue};
use crate::artifact_context::{
    ArtifactContextPolicy, ConfiguredRepositoryCatalog,
    resolve_initial_artifact_context_with_policy,
};

fn persisted_child_intent(
    repository_id: &temper_forge::RepositoryId,
    number: temper_forge::ItemNumber,
    title: &str,
    payload: &str,
) -> BTreeMap<String, CreateIssuesIntent> {
    BTreeMap::from([(
        format!("intent-{title}"),
        CreateIssuesIntent {
            transition: "fanout".into(),
            effect_index: 0,
            correlation_key: format!("intent-{title}"),
            record_parent_dependencies: true,
            children: vec![CreateIssueIntentChild {
                slug: title.into(),
                title: title.into(),
                body_hex: payload.into(),
                final_labels: vec!["code".into()],
                dependencies: vec![payload.into()],
                repository_id: repository_id.clone(),
                correlation_key: format!("child-{title}"),
                number: Some(number),
                wired: true,
                activated: true,
            }],
            completion: Some(CreateIssuesCompletion {
                body_hex: Some(payload.into()),
                add_labels: vec!["completed".into()],
                ..Default::default()
            }),
            parent_wired: true,
            completed: true,
        },
    )])
}

fn large_bookkeeping_metadata(
    kind: &str,
    parents: Vec<ArtifactRef>,
    dependencies: Vec<ArtifactRef>,
    child: Option<(&temper_forge::RepositoryId, temper_forge::ItemNumber, &str)>,
    payload: &str,
) -> WorkflowMetadata {
    let now = chrono::DateTime::from_timestamp(1, 0).unwrap();
    WorkflowMetadata {
        kind: Some(ArtifactKindId::new(kind)),
        parents,
        dependencies,
        correlation_key: Some(format!("correlation-{kind}")),
        target_branch: Some("feature/authored-context".into()),
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: payload.into(),
            claimed_at: now,
            heartbeat_at: now,
            expires_at: now,
        }),
        assignment: Some(DurableAssignment {
            coordination_key: Some(payload.into()),
            ..Default::default()
        }),
        repaired_head: Some(payload.into()),
        staged: true,
        create_issue_intents: child
            .map(|(repository_id, number, title)| {
                persisted_child_intent(repository_id, number, title, payload)
            })
            .unwrap_or_default(),
    }
}

#[test]
fn memory_forge_projects_complete_authored_lineage_without_bookkeeping() {
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
        let feature_authored = "## Objective\nFEATURE_OBJECTIVE_SENTINEL\n## Acceptance\nFEATURE_ACCEPTANCE_SENTINEL\n";
        let plan_authored =
            "## Constraints\nPLAN_CONSTRAINT_SENTINEL\n## Tests\nPLAN_TEST_SENTINEL\n";
        let code_authored = "## Implementation\nCODE_IMPLEMENTATION_SENTINEL\n## Acceptance\nCODE_ACCEPTANCE_SENTINEL\n";
        let feature = forge
            .create_issue(
                &repository.id,
                issue("feature", feature_authored.into(), &["feature"]),
            )
            .await
            .unwrap();
        let plan = forge
            .create_issue(
                &repository.id,
                issue("plan", plan_authored.into(), &["plan", "in-progress"]),
            )
            .await
            .unwrap();
        let code = forge
            .create_issue(
                &repository.id,
                issue("code", code_authored.into(), &["code", "ready"]),
            )
            .await
            .unwrap();

        let write_metadata = |authored: &str, metadata: WorkflowMetadata| {
            format!("{authored}{}", render_metadata_block(&metadata))
        };
        let payload = "private-bookkeeping-payload".repeat(4_096);
        forge
            .update_issue(
                &feature.id,
                UpdateIssue {
                    body: Some(write_metadata(
                        feature_authored,
                        large_bookkeeping_metadata(
                            "feature",
                            Vec::new(),
                            Vec::new(),
                            Some((&repository.id, plan.number, "plan child")),
                            &payload,
                        ),
                    )),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        forge
            .update_issue(
                &plan.id,
                UpdateIssue {
                    body: Some(write_metadata(
                        plan_authored,
                        large_bookkeeping_metadata(
                            "plan",
                            vec![ArtifactRef::same_repo(feature.number)],
                            vec![
                                ArtifactRef::same_repo(code.number),
                                ArtifactRef::in_repo(repository.id.clone(), code.number),
                            ],
                            Some((&repository.id, code.number, "code child")),
                            &payload,
                        ),
                    )),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        forge
            .update_issue(
                &code.id,
                UpdateIssue {
                    body: Some(write_metadata(
                        code_authored,
                        large_bookkeeping_metadata(
                            "code",
                            vec![ArtifactRef::same_repo(plan.number)],
                            Vec::new(),
                            None,
                            &payload,
                        ),
                    )),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let workflow: RawWorkflowSpec = serde_json::from_str(PLAN_WORKFLOW).unwrap();
        let workflow = workflow.validate().unwrap();
        let catalog = ConfiguredRepositoryCatalog::single(
            repository.id.clone(),
            RepositoryPath::new("ai", "temper"),
            "https://forge.example",
        );
        let policy = ArtifactContextPolicy {
            body_bytes: 512,
            bundle_bytes: 64 * 1024,
            ..ArtifactContextPolicy::default()
        };
        let bundle = resolve_initial_artifact_context_with_policy(
            &forge,
            &catalog,
            &workflow,
            &repository.id,
            ArtifactSource::Issue {
                number: code.number,
            },
            policy,
        )
        .await
        .unwrap();

        assert_eq!(bundle.primary.body, code_authored);
        assert_eq!(bundle.lineage[0].body, feature_authored);
        assert_eq!(bundle.lineage[1].body, plan_authored);
        assert!(!bundle.truncation.content_truncated);
        let serialized = serde_json::to_string(&bundle).unwrap();
        for sentinel in [
            "FEATURE_OBJECTIVE_SENTINEL",
            "FEATURE_ACCEPTANCE_SENTINEL",
            "PLAN_CONSTRAINT_SENTINEL",
            "PLAN_TEST_SENTINEL",
            "CODE_IMPLEMENTATION_SENTINEL",
            "CODE_ACCEPTANCE_SENTINEL",
        ] {
            assert!(serialized.contains(sentinel), "missing {sentinel}");
        }
        for forbidden in [
            "private-bookkeeping-payload",
            "body_hex",
            "create_issue_intents",
            "completion",
            "lease",
            "assignment",
            "repaired_head",
            "staged",
            "wired",
            "activated",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }

        let feature_workflow = bundle.lineage[0].workflow.as_ref().unwrap();
        assert_eq!(feature_workflow.kind.as_deref(), Some("feature"));
        assert_eq!(feature_workflow.children[0].number, plan.number.get());
        assert_eq!(feature_workflow.children[0].state.as_deref(), Some("open"));
        let plan_workflow = bundle.lineage[1].workflow.as_ref().unwrap();
        assert_eq!(
            plan_workflow.parents,
            [temper_protocol_worker::WorkflowArtifactReference {
                repository_id: repository.id.to_string(),
                number: feature.number.get(),
            }]
        );
        assert_eq!(plan_workflow.dependencies.len(), 1);
        assert_eq!(plan_workflow.dependencies[0].number, code.number.get());
        assert_eq!(plan_workflow.children[0].number, code.number.get());
        assert_eq!(plan_workflow.children[0].state.as_deref(), Some("open"));
        assert_eq!(
            plan_workflow.target_branch.as_deref(),
            Some("feature/authored-context")
        );
        assert_eq!(
            bundle
                .primary
                .workflow
                .as_ref()
                .unwrap()
                .correlation_key
                .as_deref(),
            Some("correlation-code")
        );

        let larger_payload = "different-private-bookkeeping".repeat(8_192);
        forge
            .update_issue(
                &plan.id,
                UpdateIssue {
                    body: Some(write_metadata(
                        plan_authored,
                        large_bookkeeping_metadata(
                            "plan",
                            vec![ArtifactRef::same_repo(feature.number)],
                            vec![ArtifactRef::same_repo(code.number)],
                            Some((&repository.id, code.number, "code child")),
                            &larger_payload,
                        ),
                    )),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let reprojection = resolve_initial_artifact_context_with_policy(
            &forge,
            &catalog,
            &workflow,
            &repository.id,
            ArtifactSource::Issue {
                number: code.number,
            },
            policy,
        )
        .await
        .unwrap();
        assert_eq!(bundle, reprojection);
    });
}
