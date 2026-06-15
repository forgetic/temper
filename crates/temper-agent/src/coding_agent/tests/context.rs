//! Parsing of the `WorkspaceContext` wire DTO temper prepares for the agent.

use super::common::*;
use crate::coding_agent::*;

#[test]
fn parses_full_context_fixture() {
    let context = parsed_fixture();
    let primary = context.primary().expect("primary repo present");
    assert_eq!(primary.id, "repo-1");
    assert_eq!(primary.owner, "acme");
    assert_eq!(primary.name, "service");
    assert_eq!(primary.default_branch, "main");
    assert_eq!(primary.dir, "service");
    assert!(primary.is_writable());
    assert_eq!(context.work_item.role, "engineer");
    assert_eq!(context.work_item.queue, "code_ready");
    assert_eq!(context.work_item.kind, "code");
    assert_eq!(context.work_item.target, "Issue { number: ItemNumber(7) }");
    assert_eq!(
        context.work_item.context,
        r#"{"artifact":{"title":"Implement docs"}}"#
    );
    assert_eq!(primary.base_branch, "main");
    assert_eq!(primary.branch_hint.as_deref(), Some("agent/pr-for-code-7"));
    assert_eq!(context.correlation_key, "pr-for-code-7");
    assert_eq!(context.checkout.as_deref(), Some("writable"));
    assert_eq!(
        context.allowed_verdicts,
        vec!["needs_architect".to_string()]
    );
    assert_eq!(
        context.guidance.role_guidance.as_deref(),
        Some("Make a real product change.")
    );
    assert_eq!(
        context.guidance.tool_guidance.as_deref(),
        Some("Use docs/product-change.md for this fixture.")
    );
    assert_eq!(
        context.guidance.tool_constraints,
        vec!["No .temper-only diffs.".to_string()]
    );
}

#[test]
fn parses_context_without_optional_guidance_and_checkout() {
    let minimal = r#"{
      "repos": [{ "id": "r", "owner": "o", "name": "n", "default_branch": "main", "dir": "n", "access": "writable", "base_branch": "main", "branch_hint": "agent/x" }],
      "work_item": { "role": "architect", "queue": "triage", "kind": "code", "target": "Issue { number: ItemNumber(1) }", "context": "{}" },
      "correlation_key": "x",
      "guidance": {}
    }"#;
    let context: WorkspaceContext = serde_json::from_str(minimal).expect("minimal context parses");
    assert_eq!(context.checkout, None);
    // A context without `allowed_verdicts` defaults to empty (back-compat with an
    // older temper that does not surface the vocabulary).
    assert!(context.allowed_verdicts.is_empty());
    assert_eq!(context.guidance, WorkspaceGuidance::default());
}
