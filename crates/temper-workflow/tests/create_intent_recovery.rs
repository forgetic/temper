//! Restart recovery coverage for durable staged child-create intents.

mod support;

use support::crash::{CrashForge, Fault, ForgeOp};
use support::{TestRoot, block_on, create_issue, new_repo};
use temper_forge::{Forge, IssueQuery};
use temper_workflow::{
    ArtifactRef, ArtifactSource, CreateIssuesChild, ExecutionContext, RawWorkflowSpec, RoleId,
    TransitionId, WorkflowMetadata, parse_metadata_block,
};

const WORKFLOW: &str = r#"{
  "name":"durable-child-create",
  "roles":[{"id":"architect"}],
  "labels":[{"id":"intake"},{"id":"planned"},{"id":"code"},{"id":"ready"}],
  "artifact_kinds":[{"id":"epic","target":"issue","identifying_labels":["intake"]}],
  "transitions":[{"id":"break_into_children","artifact":"epic","roles":["architect"],"effects":[
    {"kind":"create_issues","correlation_key":"plan-epic-1"},
    {"kind":"add_label","label":"planned"}
  ]}]
}"#;

fn metadata(body: &str) -> WorkflowMetadata {
    parse_metadata_block(body)
        .expect("metadata parses")
        .expect("metadata exists")
}

#[test]
fn startup_recovery_finishes_intent_after_child_create_crash_without_worker_context() {
    let root = TestRoot::new();
    let forge = root.forge();
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    let workflow = spec.validate().expect("workflow validates");
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let transition = TransitionId::new("break_into_children");
    let children = vec![
        CreateIssuesChild::new("api", "Define API", "Define the API.")
            .with_labels(["code", "ready"]),
        CreateIssuesChild::new("web", "Build web", "Consume the API.")
            .with_labels(["code", "ready"])
            .with_dependencies(["api"]),
    ];
    let context = ExecutionContext::new().with_create_issues_at(transition.clone(), 0, children);
    // The first child lands, but the create response is lost.
    let crashing = CrashForge::new(forge.clone(), vec![Fault::after(ForgeOp::CreateIssue, 1)]);

    block_on(workflow.executor_with_context(&crashing, context).execute(
        &repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("architect"),
    ))
    .expect_err("injected create fault interrupts apply");

    // A fresh executor reconstructs the complete set from the parent intent.
    assert_eq!(
        block_on(
            workflow
                .executor(&forge)
                .recover_create_issue_intents(&repo)
        )
        .expect("startup recovery converges"),
        1
    );
    let issues = block_on(forge.list_issues(&repo, IssueQuery::default())).expect("issues list");
    assert_eq!(issues.len(), 3, "the landed child is not duplicated");
    let parent_ref = ArtifactRef::same_repo(parent);
    let children = issues
        .iter()
        .filter(|issue| metadata(&issue.body).parents.contains(&parent_ref))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert!(children.iter().all(|issue| !metadata(&issue.body).staged));
    assert_eq!(
        metadata(
            &children
                .iter()
                .find(|issue| issue.title == "Build web")
                .unwrap()
                .body
        )
        .dependencies
        .len(),
        1
    );

    let parent_issue = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("parent lookup")
        .expect("parent exists");
    let parent_metadata = metadata(&parent_issue.body);
    assert!(
        parent_metadata
            .create_issue_intents
            .values()
            .all(|intent| intent.completed)
    );
    assert_eq!(
        block_on(
            workflow
                .executor(&forge)
                .recover_create_issue_intents(&repo)
        )
        .expect("completed replay is a no-op"),
        0
    );
}
