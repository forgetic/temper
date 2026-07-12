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
    assert!(engineer.contains("submit_for_pr"));
    assert!(engineer.contains("host responds with failure"));
    assert!(engineer.contains("needs_architect"));
    assert!(engineer.contains("needs_human"));
    assert!(engineer.contains("PR repair runs"));
    assert!(engineer.contains("updated PR `title`"));
    assert!(engineer.contains("implementation-report"));
    assert!(!engineer.contains("checkpoint(label)"));
    assert!(!engineer.contains("CHECKPOINTS:"));
    assert!(!engineer.contains("CODEBASE MEMORY"));

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
fn system_prompt_engineer_omits_checkpoint_guidance_by_default() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    assert!(!engineer.contains("checkpoint(label)"));
    assert!(!engineer.contains("checkpoint tool"));
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
fn system_prompt_renders_exact_workflow_product_contract() {
    let allowed = vec!["needs_plan".to_string(), "validated".to_string()];
    let contracts = temper_verdict::VerdictContracts::from([
        (
            "needs_plan".to_string(),
            temper_verdict::VerdictContract {
                min_children: 1,
                max_children: Some(1),
                allowed_child_kinds: vec!["plan".to_string()],
                required_child_metadata: vec!["target_branch".to_string()],
                ..Default::default()
            },
        ),
        (
            "validated".to_string(),
            temper_verdict::VerdictContract {
                max_children: Some(0),
                requires_pr_title: true,
                requires_pr_body: true,
                required_source_metadata: vec!["target_branch".to_string()],
                ..Default::default()
            },
        ),
    ]);
    let prompt = system_prompt_with_contracts(Capability::TriageWorkspace, &allowed, &contracts);
    assert!(
        prompt
            .contains("Verdict `needs_plan` requires exactly 1 child product(s) of kind(s): plan")
    );
    assert!(prompt.contains("non-blank `slug`, `title`, and `body`"));
    assert!(
        prompt.contains("Each child body must contain non-blank workflow metadata `target_branch`")
    );
    assert!(prompt.contains("`<!-- temper:workflow ... -->` JSON block"));
    assert!(prompt.contains("requires a non-blank pull-request `title`"));
    assert!(prompt.contains("requires a non-blank pull-request `body`"));
    assert!(prompt.contains("workflow metadata `target_branch`"));
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
    assert!(rendered.contains("Work item context (JSON):"));
}

#[test]
fn shared_artifact_renderer_covers_workspace_roles_and_pr_runs() {
    for role in ["architect", "engineer", "reviewer", "tester"] {
        let mut context = parsed_fixture();
        context.work_item.role = role.to_string();
        if matches!(role, "reviewer" | "tester") {
            context.work_item.target = "PullRequest { number: ItemNumber(9) }".to_string();
            context.checkout = Some("pull_request_read_only".to_string());
        }
        context.artifact_context = Some(
            serde_json::from_value(serde_json::json!({
                "version":1,
                "repository":{"id":"repo-1","path":"acme/service"},
                "artifact_type": if matches!(role, "reviewer" | "tester") { "pull_request" } else { "issue" },
                "index":[{"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":if matches!(role, "reviewer" | "tester") { "pull_request" } else { "issue" },"number":9},"title":"Primary summary","state":"open"}],
                "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
            }))
            .expect("role bundle parses"),
        );
        let rendered = user_context(&context);
        assert!(rendered.contains(&format!("Role: {role}")));
        assert!(rendered.contains("Primary artifact:"));
        assert!(rendered.contains("Body omitted from the bounded bundle"));
        assert!(rendered.contains("Forge context tools:"));
    }
}

#[test]
fn artifact_bundle_uses_shared_lineage_renderer_instead_of_legacy_json() {
    let mut context = parsed_fixture();
    context.artifact_context = Some(serde_json::from_value(serde_json::json!({
        "version": 1,
        "repository": {"id":"repo-1", "path":"acme/service"},
        "artifact_type": "issue",
        "snapshots": [
            {"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":7},"title":"Primary","body":"primary body","state":"open"},
            {"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":3},"title":"Parent","body":"parent body","state":"open"}
        ],
        "index": [
            {"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":7},"title":"Primary","state":"open","snapshot_index":0},
            {"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"pull_request","number":9},"title":"Validation PR","state":"open"},
            {"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":11},"title":"Optional reference","state":"open"}
        ],
        "relations": [
            {"relation_type":"parent","source":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":7},"target":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":3}},
            {"relation_type":"related","source":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"pull_request","number":9},"target":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":3}}
        ],
        "diagnostics":[{"code":"content_truncated","message":"body bounded"}],
        "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":true}
    })).expect("bundle parses"));

    let rendered = user_context(&context);
    for heading in [
        "Primary artifact:",
        "Mandatory lineage:",
        "Validation summaries:",
        "Optional body-omitted references:",
        "Diagnostics and truncation:",
        "Forge context tools:",
    ] {
        assert!(rendered.contains(heading), "missing {heading}");
    }
    assert!(rendered.contains("primary body"));
    assert!(rendered.contains("parent body"));
    assert!(rendered.contains("Validation PR"));
    assert!(rendered.contains("Optional reference"));
    assert!(rendered.contains("content_truncated"));
    assert!(rendered.contains("Repeated calls may follow indirect relations"));
    assert!(!rendered.contains("Work item context (JSON):"));
}
