//! Runner-level test for the `create_issues` work-product seam (ADR 0022 §C).
//!
//! [`RoleTools::run_with_create_issues_at`] is the multi-artifact counterpart of
//! the pull-request-create seam: a workspace verdict routes to a transition
//! whose `create_issues` effect fans out a plan of dependent children, and the
//! children the workspace authored are bound through the runtime seam. This test
//! drives that path end-to-end against the in-memory backend.

use temper_forge::{CreateIssue, CreateRepository, Forge, IssueQuery, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_runner::RoleTools;
use temper_testing::block_on;
use temper_workflow::{
    parse_metadata_block, ArtifactRef, ArtifactSource, CreateIssuesChild, ExecutionContext,
    RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow,
};

const WORKFLOW: &str = r#"{
    "name": "architect-fanout",
    "roles": [{"id": "architect"}],
    "labels": [{"id": "intake"}, {"id": "planned"}, {"id": "code"}, {"id": "ready"}],
    "artifact_kinds": [
        {"id": "epic", "target": "issue", "identifying_labels": ["intake"]}
    ],
    "transitions": [{"id": "break_into_children", "artifact": "epic", "roles": ["architect"], "effects": [
        {"kind": "create_issues"},
        {"kind": "add_label", "label": "planned"}
    ]}]
}"#;

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("json parses");
    spec.validate().expect("workflow validates")
}

fn repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository created")
    .id
}

#[test]
fn run_with_create_issues_fans_out_dependent_children() {
    let forge = MemoryForge::new();
    let repo = repo(&forge);
    let epic = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Epic".into(),
            body: "raw human epic".into(),
            labels: vec!["intake".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("epic created")
    .number;

    let workflow = workflow();
    let tools = RoleTools::new(
        &workflow,
        &forge,
        &repo,
        RoleId::new("architect"),
        ExecutionContext::default(),
    );

    let children = vec![
        CreateIssuesChild::new("api", "API schema", "Author the schema.")
            .with_labels(["code", "ready"]),
        CreateIssuesChild::new("web", "Web client", "Consume the schema.")
            .with_labels(["code", "ready"])
            .with_dependencies(["api"]),
    ];
    let transition = TransitionId::new("break_into_children");

    let report = block_on(tools.run_with_create_issues_at(
        ArtifactSource::Issue { number: epic },
        &transition,
        0,
        "plan-epic-1",
        children,
    ))
    .expect("create_issues seam executes");
    assert_eq!(report.target, ArtifactSource::Issue { number: epic });

    let parent_ref = ArtifactRef::same_repo(epic);
    let mut created: Vec<_> = block_on(forge.list_issues(&repo, IssueQuery::default()))
        .expect("issues list")
        .into_iter()
        .filter(|issue| {
            parse_metadata_block(&issue.body)
                .ok()
                .flatten()
                .is_some_and(|metadata| metadata.parents.contains(&parent_ref))
        })
        .collect();
    created.sort_by(|a, b| a.title.cmp(&b.title));
    assert_eq!(created.len(), 2, "two children fanned out under the epic");
    assert_eq!(created[0].title, "API schema");
    assert_eq!(created[1].title, "Web client");

    // The web child depends on the api child's number.
    let api_number = created[0].number;
    let web_meta = parse_metadata_block(&created[1].body)
        .expect("metadata parses")
        .expect("metadata exists");
    assert!(web_meta
        .dependencies
        .contains(&ArtifactRef::same_repo(api_number)));

    // The parent became the plan record: the co-declared label flip applied.
    let parent = block_on(forge.get_issue_by_number(&repo, epic))
        .expect("lookup")
        .expect("epic exists");
    assert!(parent.labels.iter().any(|label| label == "planned"));
}
