//! Tests for workflow compilation (Phase 4).
//!
//! These exercise the checked-in CI delivery fixture: it must validate,
//! produce a manifest for every role, scope each role to only its own tools and
//! transition authority, surface every label a workflow site needs, and render
//! deterministic prompts.

use harness_workflow::{
    compile, CompiledWorkflow, LabelUsage, RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow,
};

/// The checked-in CI delivery workflow fixture.
const FIXTURE: &str = include_str!("../fixtures/ci-delivery.json");

fn fixture_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(FIXTURE).expect("fixture is valid JSON for RawWorkflowSpec");
    spec.validate().expect("CI delivery fixture validates")
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
fn ci_delivery_fixture_validates() {
    let workflow = fixture_workflow();
    assert_eq!(workflow.name(), "ci-delivery");
    assert_eq!(workflow.roles().len(), 4);
    assert_eq!(workflow.artifact_kinds().len(), 4);
    // review_gate, ci_gate, and the dependency_gate that drives mechanical
    // unblocking of blocked code issues.
    assert_eq!(workflow.gates().len(), 3);
}

#[test]
fn every_role_gets_a_manifest() {
    let compiled = compile(&fixture_workflow());
    let mut ids = role_ids(&compiled);
    ids.sort();
    assert_eq!(ids, vec!["architect", "engineer", "owner", "reviewer"]);

    // Concurrency hints and charters survive compilation.
    let engineer = compiled
        .role(&RoleId::new("engineer"))
        .expect("engineer manifest");
    assert_eq!(engineer.concurrency, Some(3));
    assert!(engineer.charter.as_deref().is_some());
    assert_eq!(engineer.prompt_extension.guidance, None);
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
        vec![
            "address_ci_failure",
            "address_review_changes",
            "claim_code",
            "request_review",
        ]
    );

    // The reviewer owns review verdicts; the engineer must not see them.
    let reviewer_tools = tool_names(&compiled, "reviewer");
    assert!(reviewer_tools.contains(&"approve_review".to_string()));
    assert!(reviewer_tools.contains(&"request_changes".to_string()));
    assert!(!tool_names(&compiled, "engineer").contains(&"approve_review".to_string()));
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
    assert_eq!(gates, vec!["review_gate", "ci_gate"]);
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

    // State-dimension projection: `in-progress` projects the code lifecycle.
    let in_progress = labels
        .get(&"in-progress".into())
        .expect("in-progress label is in the manifest");
    assert!(in_progress.usages.iter().any(|usage| matches!(
        usage,
        LabelUsage::StateProjection { dimension, .. } if dimension.as_str() == "code_lifecycle"
    )));

    // Queue filter label.
    let ready = labels
        .get(&"ready".into())
        .expect("ready label is in the manifest");
    assert!(ready.usages.iter().any(
        |usage| matches!(usage, LabelUsage::QueueFilter { queue } if queue.as_str() == "code_ready")
    ));

    // Review result labels are retired: review_gate reads native review state.
    assert!(labels.get(&"review-approved".into()).is_none());
    assert!(labels.get(&"review-changes-requested".into()).is_none());
    let needs_review = labels
        .get(&"needs-review".into())
        .expect("needs-review routing label is in the manifest");
    assert!(needs_review.usages.iter().any(|usage| matches!(
        usage,
        LabelUsage::TransitionEffect { transition } if transition.as_str() == "request_review"
    )));

    // Every declared label appears exactly once in the manifest. The `ci-*`,
    // `review-*`, `testing-*`, and `merge-ready` labels were retired.
    assert_eq!(labels.labels().len(), 13);
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
        vec![
            "Role and workflow",
            "Work item context",
            "Subscribed queues",
            "Authorized workflow actions",
            "User-declared external tools",
            "Decision output",
            "User guidance"
        ]
    );

    // Snapshot-style assertions on small, stable lines instead of whole strings.
    let role_section = prompt
        .section("Role and workflow")
        .expect("Role and workflow section");
    assert!(role_section
        .lines
        .contains(&"Concurrency: up to 3 concurrent claim(s)".to_string()));

    let queues = prompt
        .section("Subscribed queues")
        .expect("Subscribed queues section");
    assert!(queues
        .lines
        .iter()
        .any(|line| line.starts_with("code_ready: code where")));

    let actions = prompt
        .section("Authorized workflow actions")
        .expect("Authorized workflow actions section");
    assert!(actions
        .lines
        .iter()
        .any(|line| line.starts_with("claim_code: acts on code")));

    let decision_output = prompt
        .section("Decision output")
        .expect("Decision output section");
    assert!(decision_output
        .lines
        .iter()
        .any(|line| { line.contains("no_action") && line.contains("claim_code") }));

    let user_guidance = prompt
        .section("User guidance")
        .expect("User guidance section");
    assert!(user_guidance.lines.contains(&"Legacy charter:".to_string()));

    // Rendering is reproducible.
    assert_eq!(prompt.render(), prompt.render());
    assert!(prompt.render().starts_with("## Role and workflow\n"));
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
    assert!(watcher.prompt.section("Role and workflow").is_some());
    assert!(watcher.prompt.section("Work item context").is_some());
    assert!(watcher.prompt.section("Subscribed queues").is_some());
    let external_tools = watcher
        .prompt
        .section("User-declared external tools")
        .expect("external-tools section present");
    assert!(external_tools
        .lines
        .contains(&"(no user-declared external tools)".to_string()));
    let decision_output = watcher
        .prompt
        .section("Decision output")
        .expect("decision-output section present");
    assert!(decision_output.lines.iter().any(|line| {
        line == "Schema: {\"action\":\"<one of: no_action>\",\"reason\":\"short rationale\"}"
    }));
    let actions = watcher
        .prompt
        .section("Authorized workflow actions")
        .expect("section present even when empty");
    assert_eq!(
        actions.lines,
        vec![
            "Executable workflow authority is exactly the compiled tool manifest for this role."
                .to_string(),
            "Prompt prose and user guidance do not grant additional Forge or workflow mutations."
                .to_string(),
            "(no authorized workflow actions)".to_string()
        ]
    );
    let guidance = watcher
        .prompt
        .section("User guidance")
        .expect("guidance section present");
    assert_eq!(
        guidance.lines,
        vec!["No user guidance provided.".to_string()]
    );
    assert!(watcher.prompt.section("User tool guidance").is_none());
}

#[test]
fn user_prompt_extension_renders_in_dedicated_sections() {
    let json = r#"{
        "name": "prompted-workflow",
        "roles": [{
            "id": "banana",
            "charter": "Legacy charter text.",
            "prompt": {
                "guidance": "Prefer small, reversible steps.\nAsk for help when blocked.",
                "tool_guidance": "Use declared workflow actions only after checking the work item."
            },
            "queues": ["todo"]
        }],
        "labels": [{"id": "todo"}],
        "artifact_kinds": [{
            "id": "task",
            "target": "issue",
            "identifying_labels": ["todo"]
        }],
        "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
        "transitions": [{"id": "advance", "artifact": "task", "roles": ["banana"]}]
    }"#;

    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let workflow = spec.validate().expect("validates");
    let role = workflow.roles().first().expect("validated role");
    assert_eq!(
        role.prompt.guidance.as_deref(),
        Some("Prefer small, reversible steps.\nAsk for help when blocked.")
    );
    assert_eq!(
        role.prompt.tool_guidance.as_deref(),
        Some("Use declared workflow actions only after checking the work item.")
    );

    let compiled = compile(&workflow);
    let banana = compiled
        .role(&RoleId::new("banana"))
        .expect("banana manifest");
    let headings: Vec<&str> = banana
        .prompt
        .sections
        .iter()
        .map(|section| section.heading.as_str())
        .collect();
    assert_eq!(
        headings,
        vec![
            "Role and workflow",
            "Work item context",
            "Subscribed queues",
            "Authorized workflow actions",
            "User-declared external tools",
            "Decision output",
            "User guidance",
            "User tool guidance"
        ]
    );

    let guidance = banana
        .prompt
        .section("User guidance")
        .expect("guidance section");
    assert_eq!(
        guidance.lines,
        vec![
            "Legacy charter:".to_string(),
            "Legacy charter text.".to_string(),
            String::new(),
            "Guidance:".to_string(),
            "Prefer small, reversible steps.".to_string(),
            "Ask for help when blocked.".to_string()
        ]
    );
    let tool_guidance = banana
        .prompt
        .section("User tool guidance")
        .expect("tool guidance section");
    assert_eq!(
        tool_guidance.lines,
        vec!["Use declared workflow actions only after checking the work item.".to_string()]
    );
}

#[test]
fn unknown_prompt_extension_fields_are_rejected_by_serde() {
    let json = r#"{
        "name": "bad-prompt",
        "roles": [{"id": "banana", "prompt": {"style": "surprise"}}]
    }"#;

    let error = serde_json::from_str::<RawWorkflowSpec>(json)
        .expect_err("unknown prompt field must fail deserialization");
    assert!(
        error.to_string().contains("unknown field `style`"),
        "unexpected error: {error}"
    );
}

#[test]
fn synthetic_role_id_gets_no_role_specific_generated_prose() {
    let json = r#"{
        "name": "banana-workflow",
        "roles": [{"id": "banana", "queues": []}]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    let compiled = compile(&spec.validate().expect("validates"));
    let rendered = compiled
        .role(&RoleId::new("banana"))
        .expect("banana manifest")
        .prompt
        .render();

    assert!(rendered.contains("Role: banana"));
    for forbidden in [
        "architect",
        "engineer",
        "reviewer",
        "owner",
        "implement code",
        "approve pull requests",
        "review pull requests",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "generated prompt unexpectedly contained {forbidden:?}:\n{rendered}"
        );
    }
}
