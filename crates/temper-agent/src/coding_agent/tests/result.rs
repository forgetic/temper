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
                labels: vec!["code".to_string()],
                depends_on: Vec::new(),
                target_repo: Some("acme/other".to_string()),
            },
            WorkspaceResultChild {
                slug: "ui".to_string(),
                title: "Add the UI".to_string(),
                body: "ui body".to_string(),
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
fn parse_result_extracts_bare_json() {
    let result = parse_result(r#"{"verdict":"approve","summary":"ok"}"#).expect("parses");
    assert_eq!(result.verdict.as_deref(), Some("approve"));
    assert_eq!(result.summary.as_deref(), Some("ok"));
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
