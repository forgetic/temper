//! `system_prompt` (role contract + verdict-vocabulary constraint) and
//! `user_context` rendering.

use super::common::*;
use crate::coding_agent::*;

#[test]
fn system_prompt_is_role_specific() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    assert!(engineer.contains("ROLE: engineer"));
    assert!(engineer.contains("product diff"));
    assert!(engineer.contains("Do NOT run git commit"));
    assert!(engineer.contains("needs_architect"));
    assert!(engineer.contains("needs_human"));
    assert!(engineer.contains("checkpoint(label)"));

    let architect = system_prompt(Capability::TriageWorkspace, &[]);
    assert!(architect.contains("ROLE: architect"));
    assert!(architect.contains("ready_code"));
    assert!(architect.contains("needs_design"));
    assert!(architect.contains("needs_breakdown"));
    assert!(architect.contains("target_repo"));

    let reviewer = system_prompt(Capability::ReviewWorkspace, &[]);
    assert!(reviewer.contains("ROLE: reviewer"));
    assert!(reviewer.contains("approve"));
    assert!(reviewer.contains("review_body"));
    assert!(reviewer.contains("git diff origin/<base_branch>...HEAD"));
    assert!(reviewer.contains("git log origin/<base_branch>..HEAD"));
    assert!(reviewer.contains("escalate"));

    // Every role must have the top-level reminder and the final-message format.
    for prompt in [engineer, architect, reviewer] {
        assert!(prompt.contains("FINAL message after all tool use"));
        assert!(prompt.contains("FINAL MESSAGE FORMAT (mandatory)"));
        assert!(prompt.contains("single JSON object"));
        assert!(!prompt.contains("publish_plan"));
        assert!(!prompt.contains("\"plan\""));
        assert!(prompt.contains("children"));
    }
}

#[test]
fn system_prompt_engineer_uses_checkpoint_only_progress_discipline() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    assert!(engineer.contains("checkpoint(label)"));
    assert!(engineer.contains("meaningful, diff-bearing"));
    assert!(engineer.contains("no up-front plan/checklist ceremony"));
    assert!(engineer.contains("validation in `summary`"));
    assert!(!engineer.contains("publish_plan"));
    assert!(!engineer.contains("`plan`"));
    assert!(!engineer.contains("phases"));

    let architect = system_prompt(Capability::TriageWorkspace, &[]);
    assert!(!architect.contains("checkpoint(label)"));
    assert!(!architect.contains("publish_plan"));
    assert!(!architect.contains("validation in `summary`"));
}

#[test]
fn system_prompt_without_allowed_verdicts_has_no_constraint_block() {
    // Back-compat: an empty vocabulary leaves the built-in per-role menu and adds
    // no constraint section.
    let architect = system_prompt(Capability::TriageWorkspace, &[]);
    assert!(!architect.contains("VERDICT CONSTRAINT"));
}

#[test]
fn system_prompt_constrains_to_allowed_verdicts() {
    // A multi-outcome triage: the constraint names exactly the declared set.
    let allowed = vec!["ready_code".to_string(), "needs_design".to_string()];
    let architect = system_prompt(Capability::TriageWorkspace, &allowed);
    assert!(architect.contains("VERDICT CONSTRAINT"));
    assert!(architect.contains("`ready_code`"));
    assert!(architect.contains("`needs_design`"));
    // It must not suggest the single-outcome collapse for a 2-element set.
    assert!(!architect.contains("SINGLE declared outcome"));
}

#[test]
fn system_prompt_single_outcome_collapses_to_one_choice() {
    // The basic-delivery architect: a single declared outcome ⇒ exactly one
    // choice. This is the deterministic single-outcome triage the example relies
    // on.
    let allowed = vec!["ready_code".to_string()];
    let architect = system_prompt(Capability::TriageWorkspace, &allowed);
    assert!(architect.contains("VERDICT CONSTRAINT"));
    assert!(architect.contains("SINGLE declared outcome"));
    assert!(architect.contains("verdict `ready_code`"));
}

#[test]
fn system_prompt_engineer_keeps_head_path_under_constraint() {
    // Even with declared decline verdicts, the engineer may still take the
    // no-verdict head path.
    let allowed = vec!["needs_architect".to_string(), "needs_human".to_string()];
    let engineer = system_prompt(Capability::CodingWorkspace, &allowed);
    assert!(engineer.contains("VERDICT CONSTRAINT"));
    assert!(engineer.contains("head path"));
    // The single-outcome collapse line is engineer-inapplicable.
    assert!(!engineer.contains("SINGLE declared outcome"));
}

#[test]
fn user_context_includes_work_item_and_guidance() {
    let context = parsed_fixture();
    let rendered = user_context(&context);
    assert!(rendered.contains("acme/service"));
    assert!(rendered.contains("dir: service/"));
    assert!(rendered.contains("access: writable"));
    assert!(rendered.contains("Role: engineer"));
    assert!(rendered.contains("Action: open_pr"));
    assert!(rendered.contains("Target: Issue { number: ItemNumber(7) }"));
    assert!(rendered.contains("base branch: main"));
    assert!(rendered.contains("work branch: agent/pr-for-code-7"));
    assert!(rendered.contains("Correlation key: pr-for-code-7"));
    assert!(rendered.contains("Checkout mode: writable"));
    assert!(rendered.contains("Make a real product change."));
    assert!(rendered.contains("Use docs/product-change.md"));
    assert!(rendered.contains("No .temper-only diffs."));
    assert!(rendered.contains(r#"{"artifact":{"title":"Implement docs"}}"#));
}
