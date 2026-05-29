//! Tests for workflow compilation (Phase 4).
//!
//! These exercise the checked-in five-role delivery fixture: it must validate,
//! produce a manifest for every role, scope each role to only its own tools and
//! transition authority, surface every label a workflow site needs, and render
//! deterministic prompts.

use harness_workflow::{
    compile, CompiledWorkflow, LabelUsage, RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow,
};

/// The checked-in five-role delivery workflow fixture.
const FIXTURE: &str = include_str!("../fixtures/five-role-delivery.json");

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON for RawWorkflowSpec");
    spec.validate().expect("five-role fixture validates")
}

fn role_ids(compiled: &CompiledWorkflow) -> Vec<String> {
    compiled
        .roles()
        .iter()
        .map(|role| role.id.to_string())
        .collect()
}

fn tool_names(compiled: &CompiledWorkflow, role: &str) -> Vec<String> {
    compiled
        .role(&RoleId::new(role))
        .expect("role manifest exists")
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

#[test]
fn five_role_fixture_validates() {
    let workflow = fixture_workflow();
    assert_eq!(workflow.name(), "five-role-delivery");
    assert_eq!(workflow.roles().len(), 5);
    assert_eq!(workflow.artifact_kinds().len(), 4);
    assert_eq!(workflow.gates().len(), 2);
}

#[test]
fn every_role_gets_a_manifest() {
    let compiled = compile(&fixture_workflow());
    let mut ids = role_ids(&compiled);
    ids.sort();
    assert_eq!(
        ids,
        vec!["architect", "engineer", "owner", "reviewer", "tester"]
    );

    // Concurrency hints and charters survive compilation.
    let engineer = compiled
        .role(&RoleId::new("engineer"))
        .expect("engineer manifest");
    assert_eq!(engineer.concurrency, Some(3));
    assert!(engineer.charter.as_deref().is_some());
}

#[test]
fn compile_method_matches_free_function() {
    let workflow = fixture_workflow();
    assert_eq!(workflow.compile(), compile(&workflow));
}

#[test]
fn each_role_sees_only_its_own_tools() {
    let compiled = compile(&fixture_workflow());

    let mut engineer_tools = tool_names(&compiled, "engineer");
    engineer_tools.sort();
    assert_eq!(
        engineer_tools,
        vec!["address_review_changes", "claim_code", "request_review"]
    );

    // The reviewer owns review verdicts; the engineer must not see them.
    let reviewer_tools = tool_names(&compiled, "reviewer");
    assert!(reviewer_tools.contains(&"approve_review".to_string()));
    assert!(reviewer_tools.contains(&"request_changes".to_string()));
    assert!(!tool_names(&compiled, "engineer").contains(&"approve_review".to_string()));
    assert!(!tool_names(&compiled, "tester").contains(&"approve_review".to_string()));

    // The tester only records test outcomes.
    let mut tester_tools = tool_names(&compiled, "tester");
    tester_tools.sort();
    assert_eq!(
        tester_tools,
        vec!["record_test_failure", "record_test_pass"]
    );
}

#[test]
fn role_authority_matches_tools_and_excludes_others() {
    let compiled = compile(&fixture_workflow());
    let owner = compiled
        .role(&RoleId::new("owner"))
        .expect("owner manifest");

    // Authority and tools describe the same set of transitions.
    let authority: Vec<String> = owner
        .authority
        .iter()
        .map(TransitionId::to_string)
        .collect();
    let tools: Vec<String> = owner
        .tools
        .iter()
        .map(|t| t.transition.to_string())
        .collect();
    assert_eq!(authority, tools);

    assert!(owner
        .authority
        .contains(&TransitionId::new("approve_merge")));
    assert!(owner
        .authority
        .contains(&TransitionId::new("clear_owner_flag")));
    // Owner never gains an engineer-only transition.
    assert!(!owner.authority.contains(&TransitionId::new("claim_code")));
}

#[test]
fn gated_tool_carries_its_required_gates() {
    let compiled = compile(&fixture_workflow());
    let owner = compiled
        .role(&RoleId::new("owner"))
        .expect("owner manifest");
    let approve_merge = owner
        .tools
        .iter()
        .find(|tool| tool.name == "approve_merge")
        .expect("owner can approve a merge");

    let gates: Vec<String> = approve_merge
        .requires_gates
        .iter()
        .map(|gate| gate.to_string())
        .collect();
    assert_eq!(gates, vec!["review_gate", "testing_gate"]);
}

#[test]
fn queue_manifests_record_subscribers() {
    let compiled = compile(&fixture_workflow());
    let code_ready = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "code_ready")
        .expect("code_ready queue is compiled");
    assert_eq!(code_ready.subscribers, vec![RoleId::new("engineer")]);
}

#[test]
fn transition_table_covers_every_transition() {
    let workflow = fixture_workflow();
    let compiled = compile(&workflow);
    assert_eq!(compiled.transitions().len(), workflow.transitions().len());
}

#[test]
fn label_manifest_covers_every_workflow_site() {
    let compiled = compile(&fixture_workflow());
    let labels = compiled.labels();

    // Identity label, from an artifact mapping.
    let code = labels
        .get(&"code".into())
        .expect("code label is in the manifest");
    assert!(code.usages.iter().any(|usage| matches!(
        usage,
        LabelUsage::ArtifactIdentity { artifact } if artifact.as_str() == "code"
    )));

    // State-dimension projection with no transition or queue (CI is external).
    let ci_passed = labels
        .get(&"ci-passed".into())
        .expect("ci-passed label is in the manifest");
    assert!(ci_passed.usages.iter().any(|usage| matches!(
        usage,
        LabelUsage::StateProjection { dimension, .. } if dimension.as_str() == "ci"
    )));

    // Queue filter label.
    let ready = labels
        .get(&"ready".into())
        .expect("ready label is in the manifest");
    assert!(ready.usages.iter().any(
        |usage| matches!(usage, LabelUsage::QueueFilter { queue } if queue.as_str() == "code_ready")
    ));

    // Gate outcome: review-approved is produced by a transition that satisfies
    // review_gate, and is also a transition effect.
    let approved = labels
        .get(&"review-approved".into())
        .expect("review-approved label is in the manifest");
    assert!(approved.usages.iter().any(|usage| matches!(
        usage,
        LabelUsage::GateOutcome { gate } if gate.as_str() == "review_gate"
    )));
    assert!(approved
        .usages
        .iter()
        .any(|usage| matches!(usage, LabelUsage::TransitionEffect { .. })));

    // Every declared label appears exactly once in the manifest.
    assert_eq!(labels.labels().len(), 22);
}

#[test]
fn prompt_sections_are_deterministic() {
    let compiled = compile(&fixture_workflow());
    let engineer = compiled
        .role(&RoleId::new("engineer"))
        .expect("engineer manifest");
    let prompt = &engineer.prompt;

    // Stable section headings in a stable order.
    let headings: Vec<&str> = prompt.sections.iter().map(|s| s.heading.as_str()).collect();
    assert_eq!(
        headings,
        vec!["Role", "Charter", "Queues", "Authorized actions"]
    );

    // Snapshot-style assertions on small, stable lines instead of whole strings.
    let role_section = prompt.section("Role").expect("Role section");
    assert!(role_section
        .lines
        .contains(&"Concurrency: up to 3 concurrent claim(s)".to_string()));

    let queues = prompt.section("Queues").expect("Queues section");
    assert!(queues
        .lines
        .iter()
        .any(|line| line.starts_with("code_ready: code where")));

    let actions = prompt
        .section("Authorized actions")
        .expect("Authorized actions section");
    assert!(actions
        .lines
        .iter()
        .any(|line| line.starts_with("claim_code: acts on code")));

    // Rendering is reproducible.
    assert_eq!(prompt.render(), prompt.render());
    assert!(prompt.render().starts_with("## Role\n"));
}

#[test]
fn role_with_no_authority_renders_empty_action_section() {
    // A minimal workflow with a role that has no transitions still compiles to a
    // well-formed prompt rather than panicking or omitting the section.
    let json = r#"{
        "name": "watcher-only",
        "roles": [{"id": "watcher", "queues": []}]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let compiled = compile(&spec.validate().expect("validates"));
    let watcher = compiled
        .role(&RoleId::new("watcher"))
        .expect("watcher manifest");
    assert!(watcher.tools.is_empty());
    let actions = watcher
        .prompt
        .section("Authorized actions")
        .expect("section present even when empty");
    assert_eq!(actions.lines, vec!["(no authorized actions)".to_string()]);
}
