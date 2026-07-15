//! Restart recovery coverage for durable staged child-create intents.

mod support;

use support::crash::{CrashForge, Fault, ForgeOp};
use support::{TestRoot, block_on, create_issue, new_repo};
use temper_forge::{CreateIssue, CreateRepository, Forge, IssueQuery, UpdateIssue};
use temper_workflow::{
    ArtifactRef, ArtifactSource, CreateIssuesChild, ExecutionContext, RawWorkflowSpec, RoleId,
    TransitionId, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};

const WORKFLOW: &str = r#"{
  "name":"durable-child-create",
  "roles":[{"id":"architect"}],
  "labels":[{"id":"intake"},{"id":"planned"},{"id":"code"},{"id":"ready"},{"id":"blocked"}],
  "artifact_kinds":[{"id":"epic","target":"issue","identifying_labels":["intake"]}],
  "state_dimensions":[{"id":"code_lifecycle","exclusive":true,"states":[
    {"id":"ready","label":"ready"},
    {"id":"blocked","label":"blocked"}
  ]}],
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

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn child_dag(count: usize, dependencies: impl Fn(usize) -> Vec<String>) -> Vec<CreateIssuesChild> {
    (0..count)
        .map(|index| {
            CreateIssuesChild::new(
                format!("child-{index}"),
                format!("Child {index}"),
                format!("Implement child {index}."),
            )
            .with_labels(["code", "ready"])
            .with_dependencies(dependencies(index))
        })
        .collect()
}

fn ten_child_dag() -> Vec<CreateIssuesChild> {
    child_dag(10, |index| {
        (0..index)
            .map(|dependency| format!("child-{dependency}"))
            .collect()
    })
}

#[test]
fn startup_recovery_finishes_intent_after_child_create_crash_without_worker_context() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
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

    assert_eq!(
        crashing.count(ForgeOp::ListIssues),
        0,
        "known-first create must not scan correlation history"
    );
    let staged_after_uncertain_create =
        block_on(forge.list_issues(&repo, IssueQuery::default())).expect("staged inventory");
    let staged_child = staged_after_uncertain_create
        .iter()
        .find(|issue| issue.number != parent)
        .expect("uncertain create landed one child");
    assert_eq!(staged_child.labels, vec!["code", "ready"]);
    assert!(metadata(&staged_child.body).staged);

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
    let web = children
        .iter()
        .find(|issue| issue.title == "Build web")
        .unwrap();
    assert_eq!(web.labels, vec!["blocked", "code"]);
    assert_eq!(metadata(&web.body).dependencies.len(), 1);

    let parent_issue = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("parent lookup")
        .expect("parent exists");
    let parent_metadata = metadata(&parent_issue.body);
    assert!(parent_issue.labels.iter().any(|label| label == "planned"));
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

#[test]
fn known_first_ten_child_dag_uses_pass_level_writes_and_no_history_scan() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["intake"], "ten-child parent");
    let transition = TransitionId::new("break_into_children");
    let children = ten_child_dag();
    let context = ExecutionContext::new().with_create_issues_at(transition.clone(), 0, children);
    let counted = CrashForge::new(forge.clone(), vec![]);

    block_on(workflow.executor_with_context(&counted, context).execute(
        &repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("architect"),
    ))
    .expect("ten-child fan-out completes");

    assert_eq!(counted.count(ForgeOp::ListIssues), 0);
    assert_eq!(counted.count(ForgeOp::CreateIssue), 10);
    assert_eq!(
        counted.count(ForgeOp::UpdateIssue),
        23,
        "1 intent + 1 create checkpoint + 9 dependent-child writes + 1 aggregation + 10 activations + 1 completion"
    );

    let issues = block_on(forge.list_issues(&repo, IssueQuery::default())).expect("inventory");
    assert_eq!(issues.len(), 11);
    for index in 0..10 {
        let child = issues
            .iter()
            .find(|issue| issue.title == format!("Child {index}"))
            .expect("child exists");
        let child_metadata = metadata(&child.body);
        assert!(!child_metadata.staged);
        assert_eq!(child_metadata.dependencies.len(), index);
        let expected_labels = if index == 0 {
            vec!["code", "ready"]
        } else {
            vec!["blocked", "code"]
        };
        assert_eq!(child.labels, expected_labels);
    }
}

#[test]
fn known_first_core_operations_follow_child_and_dependent_child_formula() {
    let cases = vec![
        ("zero", child_dag(0, |_| Vec::new()), 0usize),
        ("one", child_dag(1, |_| Vec::new()), 0),
        (
            "ten sparse",
            child_dag(10, |index| {
                [3usize, 9]
                    .contains(&index)
                    .then(|| vec!["child-0".to_string()])
                    .unwrap_or_default()
            }),
            2,
        ),
        (
            "ten chain",
            child_dag(10, |index| {
                (index > 0)
                    .then(|| vec![format!("child-{}", index - 1)])
                    .unwrap_or_default()
            }),
            9,
        ),
        ("ten maximal", ten_child_dag(), 9),
    ];

    let workflow = workflow();
    let transition = TransitionId::new("break_into_children");
    for (name, children, dependent_children) in cases {
        let root = TestRoot::new();
        let forge = root.forge();
        let repo = new_repo(&forge);
        let parent = create_issue(&forge, &repo, &["intake"], name);
        let child_count = children.len();
        let context =
            ExecutionContext::new().with_create_issues_at(transition.clone(), 0, children);
        let counted = CrashForge::new(forge, vec![]);

        block_on(workflow.executor_with_context(&counted, context).execute(
            &repo,
            ArtifactSource::Issue { number: parent },
            &transition,
            &RoleId::new("architect"),
        ))
        .unwrap_or_else(|error| panic!("{name}: fan-out failed: {error}"));

        let writes = counted.count(ForgeOp::CreateIssue) + counted.count(ForgeOp::UpdateIssue);
        let ceiling = 4 + 2 * child_count + dependent_children;
        assert!(
            writes <= ceiling,
            "{name}: {writes} writes exceeded {ceiling}"
        );
        if child_count > 0 {
            assert_eq!(writes, ceiling, "{name}: operation formula drifted");
        }
        let reads = counted.count(ForgeOp::GetIssue)
            + counted.count(ForgeOp::GetIssueByNumber)
            + counted.count(ForgeOp::ListIssues);
        assert!(
            reads <= 10,
            "{name}: {reads} core reads exceeded accepted budget"
        );
        assert!(writes <= 34, "{name}: accepted write budget exceeded");
        assert!(
            counted
                .issue_exact_details()
                .iter()
                .all(|details| *details == temper_forge::ItemListDetails::summary()),
            "{name}: metadata-only fan-out requested dependency enrichment"
        );
    }
}

#[test]
fn known_first_multi_repository_grouping_keeps_the_same_write_formula() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let parent_repo = new_repo(&forge);
    let target_repo = block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "budget-target".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .unwrap()
    .id;
    let parent = create_issue(&forge, &parent_repo, &["intake"], "multi-repo budget");
    let transition = TransitionId::new("break_into_children");
    let children = vec![
        CreateIssuesChild::new("a", "A", "A").with_labels(["code", "ready"]),
        CreateIssuesChild::new("b", "B", "B")
            .with_labels(["code", "ready"])
            .with_target_repo(target_repo.clone())
            .with_dependencies(["a"]),
        CreateIssuesChild::new("c", "C", "C")
            .with_labels(["code", "ready"])
            .with_target_repo(target_repo),
        CreateIssuesChild::new("d", "D", "D")
            .with_labels(["code", "ready"])
            .with_dependencies(["c"]),
    ];
    let context = ExecutionContext::new().with_create_issues_at(transition.clone(), 0, children);
    let counted = CrashForge::new(forge, vec![]);

    block_on(workflow.executor_with_context(&counted, context).execute(
        &parent_repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("architect"),
    ))
    .unwrap();

    assert_eq!(counted.count(ForgeOp::ListIssues), 0);
    assert_eq!(counted.count(ForgeOp::CreateIssue), 4);
    assert_eq!(counted.count(ForgeOp::UpdateIssue), 10);
    assert_eq!(4 + 10, 4 + 2 * 4 + 2);
}

#[test]
fn completion_atomically_preserves_intent_in_the_routed_parent_body_update() {
    let root = TestRoot::new();
    let forge = root.forge();
    let body_workflow = WORKFLOW.replace(
        r#"{"kind":"add_label","label":"planned"}"#,
        r#"{"kind":"set_body","correlation_key":"parent-body"},{"kind":"add_label","label":"planned"}"#,
    );
    let spec: RawWorkflowSpec = serde_json::from_str(&body_workflow).expect("workflow parses");
    let workflow = spec.validate().expect("workflow validates");
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["intake"], "old body");
    let transition = TransitionId::new("break_into_children");
    let authored = "new parent prose with --> inside";
    let context = ExecutionContext::new()
        .with_create_issues_at(
            transition.clone(),
            0,
            vec![
                CreateIssuesChild::new("only", "Only child", "child body")
                    .with_labels(["code", "ready"]),
            ],
        )
        .with_set_body_at(transition.clone(), 0, authored);

    block_on(workflow.executor_with_context(&forge, context).execute(
        &repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("architect"),
    ))
    .expect("fan-out and routed body update complete");

    let committed = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("parent lookup")
        .expect("parent exists");
    assert!(committed.body.starts_with(authored));
    assert!(committed.labels.iter().any(|label| label == "planned"));
    assert!(
        metadata(&committed.body)
            .create_issue_intents
            .values()
            .all(|intent| intent.completed && intent.children.iter().all(|child| child.activated))
    );
}

#[test]
fn ten_child_dag_converges_across_every_uncertain_pass_mutation() {
    let mut cases = Vec::new();
    for occurrence in 1..=10 {
        cases.push((
            format!("before create {occurrence}"),
            Fault::before(ForgeOp::CreateIssue, occurrence),
        ));
        cases.push((
            format!("after create {occurrence}"),
            Fault::after(ForgeOp::CreateIssue, occurrence),
        ));
    }
    // Update #1 persists the intent and deliberately precedes the tested
    // passes. #2 is the create-pass checkpoint, #3..#11 are the nine batched
    // child wiring writes, #12 is parent aggregation, #13..#22 are activation,
    // and #23 atomically completes the intent and source transition.
    for occurrence in 2..=23 {
        cases.push((
            format!("before pass update {occurrence}"),
            Fault::before(ForgeOp::UpdateIssue, occurrence),
        ));
        cases.push((
            format!("after pass update {occurrence}"),
            Fault::after(ForgeOp::UpdateIssue, occurrence),
        ));
    }

    let workflow = workflow();
    let transition = TransitionId::new("break_into_children");
    for (case, fault) in cases {
        let root = TestRoot::new();
        let forge = root.forge();
        let repo = new_repo(&forge);
        let parent = create_issue(&forge, &repo, &["intake"], "faulted ten-child parent");
        let context =
            ExecutionContext::new().with_create_issues_at(transition.clone(), 0, ten_child_dag());
        let crashing = CrashForge::new(forge.clone(), vec![fault]);
        assert!(
            block_on(workflow.executor_with_context(&crashing, context).execute(
                &repo,
                ArtifactSource::Issue { number: parent },
                &transition,
                &RoleId::new("architect"),
            ))
            .is_err(),
            "{case}: fault should interrupt execution"
        );

        block_on(
            workflow
                .executor(&crashing)
                .recover_create_issue_intents(&repo),
        )
        .unwrap_or_else(|error| panic!("{case}: recovery failed: {error}"));
        let issues = block_on(forge.list_issues(&repo, IssueQuery::default()))
            .unwrap_or_else(|error| panic!("{case}: inventory failed: {error}"));
        assert_eq!(issues.len(), 11, "{case}: duplicate or missing child");
        let titles = issues
            .iter()
            .filter(|issue| issue.number != parent)
            .map(|issue| issue.title.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(titles.len(), 10, "{case}: child titles are not unique");
        for index in 0..10 {
            let child = issues
                .iter()
                .find(|issue| issue.title == format!("Child {index}"))
                .unwrap_or_else(|| panic!("{case}: child {index} missing"));
            let child_metadata = metadata(&child.body);
            assert!(
                !child_metadata.staged,
                "{case}: child {index} stayed staged"
            );
            assert_eq!(
                child_metadata.dependencies.len(),
                index,
                "{case}: child {index} dependency set is incomplete"
            );
            assert_eq!(
                child_metadata
                    .dependencies
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                index,
                "{case}: child {index} contains duplicate dependencies"
            );
        }
        let parent_issue = issues
            .iter()
            .find(|issue| issue.number == parent)
            .expect("parent remains");
        assert!(
            metadata(&parent_issue.body)
                .create_issue_intents
                .values()
                .all(|intent| intent.completed),
            "{case}: parent intent did not complete"
        );
        let reads = crashing.count(ForgeOp::GetIssue)
            + crashing.count(ForgeOp::GetIssueByNumber)
            + crashing.count(ForgeOp::ListIssues);
        let writes = crashing.count(ForgeOp::CreateIssue) + crashing.count(ForgeOp::UpdateIssue);
        assert!(reads <= 30, "{case}: crash replay used {reads} reads");
        assert!(writes <= 68, "{case}: crash replay used {writes} writes");
    }
}

#[test]
fn recovery_groups_unresolved_correlations_by_target_repository() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let parent_repo = new_repo(&forge);
    let target_repo = block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "target".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("target repository exists")
    .id;
    let parent = create_issue(&forge, &parent_repo, &["intake"], "cross-repo parent");
    let transition = TransitionId::new("break_into_children");
    let children = vec![
        CreateIssuesChild::new("parent-a", "Parent A", "parent A").with_labels(["code", "ready"]),
        CreateIssuesChild::new("target-a", "Target A", "target A")
            .with_labels(["code", "ready"])
            .with_target_repo(target_repo.clone())
            .with_dependencies(["parent-a"]),
        CreateIssuesChild::new("parent-b", "Parent B", "parent B")
            .with_labels(["code", "ready"])
            .with_dependencies(["target-a"]),
        CreateIssuesChild::new("target-b", "Target B", "target B")
            .with_labels(["code", "ready"])
            .with_target_repo(target_repo.clone())
            .with_dependencies(["parent-b"]),
    ];
    let context = ExecutionContext::new().with_create_issues_at(transition.clone(), 0, children);
    let crashing = CrashForge::new(forge.clone(), vec![Fault::after(ForgeOp::CreateIssue, 1)]);
    block_on(workflow.executor_with_context(&crashing, context).execute(
        &parent_repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("architect"),
    ))
    .expect_err("first create response is uncertain");

    let recovery = CrashForge::new(forge.clone(), vec![]);
    assert_eq!(
        block_on(
            workflow
                .executor(&recovery)
                .recover_create_issue_intents(&parent_repo)
        )
        .expect("cross-repository recovery converges"),
        1
    );
    let correlation_queries = recovery
        .issue_queries()
        .into_iter()
        .filter(|query| query.body_contains.as_deref() == Some("\"correlation_key\""))
        .collect::<Vec<_>>();
    assert_eq!(
        correlation_queries.len(),
        4,
        "two states are queried once for each of two target repositories"
    );
    assert_eq!(
        correlation_queries
            .iter()
            .filter(|query| query.state == Some(temper_forge::IssueState::Open))
            .count(),
        2
    );
    assert!(
        correlation_queries
            .iter()
            .all(|query| query.details == temper_forge::ItemListDetails::summary())
    );

    let parent_issues =
        block_on(forge.list_issues(&parent_repo, IssueQuery::default())).expect("parent inventory");
    let target_issues =
        block_on(forge.list_issues(&target_repo, IssueQuery::default())).expect("target inventory");
    assert_eq!(parent_issues.len(), 3);
    assert_eq!(target_issues.len(), 2);
    assert!(
        parent_issues
            .iter()
            .chain(target_issues.iter())
            .filter(|issue| issue.number != parent || issue.repo_id != parent_repo)
            .all(|issue| !metadata(&issue.body).staged)
    );
}

#[test]
fn legacy_boolean_intent_recovers_without_a_persisted_completion_update() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["intake"], "legacy parent");
    let correlation = "legacy-child";
    let child_body = format!(
        "legacy child\n\n{}",
        render_metadata_block(&WorkflowMetadata {
            parents: vec![ArtifactRef::same_repo(parent)],
            correlation_key: Some(correlation.into()),
            staged: true,
            ..WorkflowMetadata::default()
        })
    );
    let child = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Legacy child".into(),
            body: child_body,
            labels: Vec::new(),
            assignees: Vec::new(),
        },
    ))
    .expect("legacy staged child exists");
    let parent_issue = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("parent lookup")
        .expect("parent exists");
    let legacy_metadata = serde_json::json!({
        "create_issue_intents": {
            "legacy": {
                "transition": "break_into_children",
                "effect_index": 0,
                "correlation_key": "legacy",
                "children": [{
                    "slug": "legacy-child",
                    "title": "Legacy child",
                    "body_hex": "6c6567616379206368696c64",
                    "final_labels": ["code", "ready"],
                    "dependencies": [],
                    "repository_id": repo.as_str(),
                    "correlation_key": correlation,
                    "number": child.number,
                    "wired": false,
                    "activated": false
                }],
                "parent_wired": false,
                "completed": false
            }
        }
    });
    block_on(forge.update_issue(
        &parent_issue.id,
        UpdateIssue {
            body: Some(format!(
                "legacy parent\n\n<!-- temper:workflow\n{}\n-->",
                serde_json::to_string_pretty(&legacy_metadata).unwrap()
            )),
            expected_version: Some(parent_issue.version),
            ..UpdateIssue::default()
        },
    ))
    .expect("legacy intent persisted");

    assert_eq!(
        block_on(
            workflow
                .executor(&forge)
                .recover_create_issue_intents(&repo)
        )
        .expect("legacy intent recovery converges"),
        1
    );
    let recovered_child = block_on(forge.get_issue_by_number(&repo, child.number))
        .expect("child lookup")
        .expect("child exists");
    assert_eq!(recovered_child.labels, vec!["code", "ready"]);
    assert!(!metadata(&recovered_child.body).staged);
    let recovered_parent = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("parent lookup")
        .expect("parent exists");
    let legacy = &metadata(&recovered_parent.body).create_issue_intents["legacy"];
    assert!(legacy.completed);
    assert!(legacy.completion.is_none());
}
