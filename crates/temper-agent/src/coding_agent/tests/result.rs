//! `WorkspaceResult` serialization, reply parsing, and the per-role contract
//! plus verdict-vocabulary validation.

use super::common::*;
use crate::coding_agent::*;

#[test]
fn head_path_result_serializes_without_verdict_or_empty_fields() {
    let result = WorkspaceResult {
        summary: Some("Implemented the banner greeting.".to_string()),
        ..WorkspaceResult::default()
    };
    let json = serde_json::to_string(&result).expect("serializes");
    assert_eq!(json, r#"{"summary":"Implemented the banner greeting."}"#);
    // Round-trips through temper's shape (deny_unknown_fields would reject extras).
    let back: WorkspaceResult = serde_json::from_str(&json).expect("round trips");
    assert_eq!(back, result);
}

#[test]
fn breakdown_result_serializes_children() {
    let result = WorkspaceResult {
        verdict: Some("needs_breakdown".to_string()),
        children: vec![
            WorkspaceResultChild {
                slug: "api".to_string(),
                title: "Add the API".to_string(),
                body: "api body".to_string(),
                kind: None,
                labels: vec!["code".to_string()],
                depends_on: Vec::new(),
                target_repo: Some("acme/other".to_string()),
            },
            WorkspaceResultChild {
                slug: "ui".to_string(),
                title: "Add the UI".to_string(),
                body: "ui body".to_string(),
                kind: None,
                labels: Vec::new(),
                depends_on: vec!["api".to_string()],
                target_repo: None,
            },
        ],
        ..WorkspaceResult::default()
    };
    let value = serde_json::to_value(&result).expect("serializes");
    assert_eq!(value["verdict"], "needs_breakdown");
    assert_eq!(value["children"][0]["slug"], "api");
    assert_eq!(value["children"][0]["target_repo"], "acme/other");
    assert_eq!(value["children"][1]["depends_on"][0], "api");
    assert!(value["children"][1].get("target_repo").is_none());
    // No spurious head-path fields.
    assert!(value.get("summary").is_none());
    assert!(value.get("body").is_none());
}

#[test]
fn parse_result_ignores_legacy_structured_plan_from_json() {
    let text = r#"{
        "summary": "implemented the change",
        "plan": {"phases": ["extend DTO", "update prompt"]}
    }"#;
    let result = parse_result(text).expect("parses legacy plan field");
    assert_eq!(result.summary.as_deref(), Some("implemented the change"));
}

#[test]
fn parse_result_extracts_bare_json() {
    let result = parse_result(r#"{"verdict":"approve","summary":"ok"}"#).expect("parses");
    assert_eq!(result.verdict.as_deref(), Some("approve"));
    assert_eq!(result.summary.as_deref(), Some("ok"));
}

#[test]
fn parse_result_does_not_parse_prose_checklist_as_plan() {
    let text = "Plan:\n- [ ] extend DTO\n- [ ] update prompt\n\
                Final result: {\"summary\":\"implemented the change\"}";
    let result = parse_result(text).expect("parses final JSON only");
    assert_eq!(result.summary.as_deref(), Some("implemented the change"));
}

#[test]
fn parse_result_tolerates_code_fence_and_prose() {
    let text =
        "Here is the result:\n```json\n{\"verdict\": \"ready_code\", \"body\": \"spec\"}\n```\n";
    let result = parse_result(text).expect("parses");
    assert_eq!(result.verdict.as_deref(), Some("ready_code"));
    assert_eq!(result.body.as_deref(), Some("spec"));
}

#[test]
fn parse_result_empty_reply_is_empty_head_path() {
    let result = parse_result("   \n").expect("empty reply parses as default");
    assert_eq!(result, WorkspaceResult::default());
}

#[test]
fn parse_result_rejects_unparseable_prose() {
    let error = parse_result("I could not finish the task.").expect_err("no JSON object");
    assert!(matches!(error, CodingAgentError::Parse { .. }));
}

#[test]
fn parse_result_skips_stray_prose_brace_before_result() {
    // Regression for the #226 architect failure: the model narrated the change
    // with an inline `tracing::info!{…}`-style brace span ahead of the real
    // result envelope. The first balanced `{…}` is not the result, so a
    // first-object parse fails at "key must be a string"; we must pick the last
    // object that matches the result shape instead.
    let text = "The change replaces info!{event: \"x\"} with debug!{event: \"x\"}. \
                Result:\n{\"verdict\":\"ready_code\",\"body\":\"spec\"}";
    let result = parse_result(text).expect("parses the real result, not the prose brace");
    assert_eq!(result.verdict.as_deref(), Some("ready_code"));
    assert_eq!(result.body.as_deref(), Some("spec"));
}

#[test]
fn parse_result_prefers_last_matching_object() {
    // Two schema-valid objects: the last one wins (the result is emitted last).
    let text = "{\"summary\":\"draft\"}\nfinal:\n{\"verdict\":\"approve\",\"summary\":\"final\"}";
    let result = parse_result(text).expect("parses");
    assert_eq!(result.verdict.as_deref(), Some("approve"));
    assert_eq!(result.summary.as_deref(), Some("final"));
}

#[test]
fn parse_result_keeps_children_object_intact() {
    // A nested object (children[]) must stay a single candidate, not be split
    // into the inner child brace.
    let text = "{\"verdict\":\"needs_breakdown\",\"children\":\
                [{\"slug\":\"api\",\"title\":\"Add API\",\"body\":\"b\"}]}";
    let result = parse_result(text).expect("parses nested object as one candidate");
    assert_eq!(result.verdict.as_deref(), Some("needs_breakdown"));
    assert_eq!(result.children.len(), 1);
    assert_eq!(result.children[0].slug, "api");
}

#[test]
fn validate_workflow_verdict_contract_rejects_missing_and_malformed_children() {
    let contracts = temper_verdict::VerdictContracts::from([(
        "needs_plan".to_string(),
        temper_verdict::VerdictContract {
            min_children: 1,
            max_children: Some(1),
            allowed_child_kinds: vec!["plan".to_string()],
            ..Default::default()
        },
    )]);
    let missing = WorkspaceResult {
        verdict: Some("needs_plan".to_string()),
        ..Default::default()
    };
    let error = crate::coding_agent::result::validate_verdict_contract(
        &missing,
        &contracts,
        &Default::default(),
    )
    .expect_err("missing child is rejected");
    assert!(matches!(error, CodingAgentError::InvalidVerdictResult(_)));

    let malformed = WorkspaceResult {
        verdict: Some("needs_plan".to_string()),
        children: vec![WorkspaceResultChild {
            slug: " ".to_string(),
            title: "".to_string(),
            body: "".to_string(),
            kind: Some("code".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let error = crate::coding_agent::result::validate_verdict_contract(
        &malformed,
        &contracts,
        &Default::default(),
    )
    .expect_err("malformed child is rejected")
    .to_string();
    assert!(error.contains("blank slug"));
    assert!(error.contains("blank title"));
    assert!(error.contains("blank body"));
    assert!(error.contains("allowed kinds: plan"));
}

#[test]
fn validate_contract_engineer_requires_diff_or_verdict() {
    // No diff, no verdict ⇒ NoProduct. Use a temp dir that is not a git repo so
    // `git status` fails and `working_tree_has_changes` returns false.
    let temp = std::env::temp_dir().join(format!("anvil-agent-test-{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir");
    let empty = WorkspaceResult::default();
    // The writable repo's dir resolves to the non-git temp dir, so there is no
    // product.
    let context = context_with_writable_dir("");
    let error = validate_contract(Capability::CodingWorkspace, &empty, &temp, &context)
        .expect_err("no product");
    assert!(matches!(error, CodingAgentError::NoProduct));

    // A verdict (needs_architect) satisfies the contract even with no diff.
    let with_verdict = WorkspaceResult {
        verdict: Some("needs_architect".to_string()),
        ..WorkspaceResult::default()
    };
    validate_contract(Capability::CodingWorkspace, &with_verdict, &temp, &context)
        .expect("verdict satisfies engineer contract");
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn validate_contract_readonly_requires_verdict() {
    let cwd = std::env::temp_dir();
    let context = context_with_writable_dir("");
    let no_verdict = WorkspaceResult {
        summary: Some("looked around".to_string()),
        ..WorkspaceResult::default()
    };
    assert!(matches!(
        validate_contract(Capability::TriageWorkspace, &no_verdict, &cwd, &context),
        Err(CodingAgentError::AgentStopped(_))
    ));
    assert!(matches!(
        validate_contract(Capability::ReviewWorkspace, &no_verdict, &cwd, &context),
        Err(CodingAgentError::AgentStopped(_))
    ));

    let approved = WorkspaceResult {
        verdict: Some("approve".to_string()),
        ..WorkspaceResult::default()
    };
    validate_contract(Capability::ReviewWorkspace, &approved, &cwd, &context)
        .expect("verdict satisfies reviewer contract");
}

#[test]
fn validate_verdict_vocabulary_accepts_declared_verdict() {
    let allowed = vec!["ready_code".to_string()];
    let result = WorkspaceResult {
        verdict: Some("ready_code".to_string()),
        body: Some("spec".to_string()),
        ..WorkspaceResult::default()
    };
    validate_verdict_vocabulary(&result, &allowed).expect("declared verdict passes");
}

#[test]
fn validate_verdict_vocabulary_rejects_undeclared_verdict() {
    // The single-outcome basic-delivery triage: a `needs_design` from the model
    // is rejected before temper would fail the tick.
    let allowed = vec!["ready_code".to_string()];
    let result = WorkspaceResult {
        verdict: Some("needs_design".to_string()),
        ..WorkspaceResult::default()
    };
    let error =
        validate_verdict_vocabulary(&result, &allowed).expect_err("undeclared verdict rejected");
    match error {
        CodingAgentError::UndeclaredVerdict { emitted, allowed } => {
            assert_eq!(emitted, "needs_design");
            assert_eq!(allowed, vec!["ready_code".to_string()]);
        }
        other => panic!("expected UndeclaredVerdict, got {other:?}"),
    }
}

#[test]
fn validate_verdict_vocabulary_allows_head_path_and_empty_vocabulary() {
    let allowed = vec!["ready_code".to_string()];
    // No verdict (head path) passes even when a vocabulary is declared.
    let head = WorkspaceResult {
        summary: Some("left a diff".to_string()),
        ..WorkspaceResult::default()
    };
    validate_verdict_vocabulary(&head, &allowed).expect("head path passes");

    // An empty vocabulary (older temper / no declared outcomes) skips the check.
    let any_verdict = WorkspaceResult {
        verdict: Some("anything".to_string()),
        ..WorkspaceResult::default()
    };
    validate_verdict_vocabulary(&any_verdict, &[]).expect("empty vocabulary skips the check");
}
