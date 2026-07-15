//! Regression coverage for repeated logical executions of one durable fan-out.

mod support;

use support::{TestRoot, block_on, close_issue, create_issue, new_repo};
use temper_forge::{Forge, IssueQuery, UserId};
use temper_workflow::{
    ArtifactRef, ArtifactSource, CreateIssuesChild, ExecutionContext, RawWorkflowSpec, RoleId,
    TransitionId, parse_metadata_block,
};

const WORKFLOW: &str = r#"{
  "name":"repeated-validation-followup",
  "roles":[{"id":"tester"},{"id":"mechanical"}],
  "labels":[
    {"id":"plan"},{"id":"needs-validation"},{"id":"in-progress"},
    {"id":"code"},{"id":"ready"}
  ],
  "artifact_kinds":[
    {"id":"plan","target":"issue","identifying_labels":["plan"]},
    {"id":"code","target":"issue","identifying_labels":["code"]}
  ],
  "relations":[{"kind":"dependency","source":"plan","target":"code"}],
  "gates":[{"id":"dependency_gate","condition":{"kind":"dependencies_resolved"}}],
  "transitions":[
    {
      "id":"plan_validation_needs_followup",
      "artifact":"plan",
      "roles":["tester"],
      "effects":[
        {"kind":"create_issues","correlation_key":"validation-followup","record_parent_dependencies":true},
        {"kind":"remove_label","label":"needs-validation"},
        {"kind":"add_label","label":"in-progress"},
        {"kind":"set_assignee","role":"tester"}
      ]
    },
    {
      "id":"mark_plan_needs_validation",
      "artifact":"plan",
      "roles":["mechanical"],
      "requires_gates":["dependency_gate"],
      "effects":[
        {"kind":"remove_label","label":"in-progress"},
        {"kind":"add_label","label":"needs-validation"}
      ]
    }
  ]
}"#;

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn followup(slug: &str, title: &str, body: &str) -> CreateIssuesChild {
    CreateIssuesChild::new(slug, title, body).with_labels(["code", "ready"])
}

#[test]
fn a_second_validation_followup_round_creates_and_commits_new_children() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["plan", "needs-validation"], "feature plan");
    let target = ArtifactSource::Issue { number: parent };
    let followup_transition = TransitionId::new("plan_validation_needs_followup");
    let tester = RoleId::new("tester");
    let first_context = ExecutionContext::new()
        .with_assignee(tester.clone(), UserId::new("tester"))
        .with_create_issues_at(
            followup_transition.clone(),
            0,
            [followup(
                "first-gap",
                "Fix first gap",
                "first round content",
            )],
        );

    block_on(
        workflow
            .executor_with_context(&forge, first_context)
            .execute(&repo, target, &followup_transition, &tester),
    )
    .expect("first follow-up round commits");

    let first_inventory =
        block_on(forge.list_issues(&repo, IssueQuery::default())).expect("first inventory loads");
    let first_child = first_inventory
        .iter()
        .find(|issue| issue.title == "Fix first gap")
        .expect("first child exists");
    assert_eq!(first_inventory.len(), 2);
    close_issue(&forge, &repo, first_child.number);

    block_on(workflow.executor(&forge).execute(
        &repo,
        target,
        &TransitionId::new("mark_plan_needs_validation"),
        &RoleId::new("mechanical"),
    ))
    .expect("completed dependency returns the plan to validation");

    let second_context = ExecutionContext::new()
        .with_assignee(tester.clone(), UserId::new("tester"))
        .with_create_issues_at(
            followup_transition.clone(),
            0,
            [followup(
                "second-gap",
                "Fix second gap",
                "different second round content",
            )],
        );
    block_on(
        workflow
            .executor_with_context(&forge, second_context)
            .execute(&repo, target, &followup_transition, &tester),
    )
    .expect("second follow-up round commits without a postcondition failure");

    let inventory =
        block_on(forge.list_issues(&repo, IssueQuery::default())).expect("final inventory loads");
    assert_eq!(inventory.len(), 3, "the second round created a new child");
    let second_child = inventory
        .iter()
        .find(|issue| issue.title == "Fix second gap")
        .expect("second child exists");
    let first_metadata = parse_metadata_block(&first_child.body)
        .expect("first child metadata parses")
        .expect("first child metadata exists");
    let second_metadata = parse_metadata_block(&second_child.body)
        .expect("second child metadata parses")
        .expect("second child metadata exists");
    assert_ne!(
        first_metadata.correlation_key, second_metadata.correlation_key,
        "child correlation identity distinguishes logical rounds"
    );

    let parent_issue = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("parent reload succeeds")
        .expect("parent exists");
    assert!(parent_issue.labels.contains(&"plan".to_string()));
    assert!(parent_issue.labels.contains(&"in-progress".to_string()));
    assert!(
        !parent_issue
            .labels
            .contains(&"needs-validation".to_string())
    );
    let parent_metadata = parse_metadata_block(&parent_issue.body)
        .expect("parent metadata parses")
        .expect("parent metadata exists");
    assert_eq!(
        parent_metadata.dependencies,
        vec![
            ArtifactRef::same_repo(first_child.number),
            ArtifactRef::same_repo(second_child.number),
        ]
    );
    assert_eq!(parent_metadata.create_issue_intents.len(), 2);
    assert!(
        parent_metadata
            .create_issue_intents
            .values()
            .all(|intent| intent.completed)
    );
}
