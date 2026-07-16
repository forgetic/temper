//! `system_prompt` (invariant role contract + workflow/fallback outcomes) and
//! `user_context` authority rendering.

use super::common::*;
use crate::coding_agent::*;

#[test]
fn system_prompt_is_role_specific() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    assert!(engineer.contains("ROLE: engineer"));
    assert!(engineer.contains("product diff"));
    assert!(engineer.contains("Do NOT run git commit"));
    assert!(!engineer.contains("submit_for_pr"));
    assert!(!engineer.contains("investigate"));
    assert!(engineer.contains("needs_architect"));
    assert!(engineer.contains("needs_human"));
    assert!(engineer.contains("PR repair runs"));
    assert!(engineer.contains("updated PR `title`"));
    assert!(engineer.contains("implementation-report"));
    assert!(!engineer.contains("checkpoint(label)"));
    assert!(!engineer.contains("CHECKPOINTS:"));
    assert!(!engineer.contains("CODEBASE MEMORY"));
    assert!(!engineer.contains("`write`"));
    assert!(!engineer.contains("`edit`"));

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
fn system_prompt_without_allowed_verdicts_uses_legacy_fallback() {
    // Back-compat: an empty vocabulary retains the built-in per-role menu.
    let architect = system_prompt(Capability::TriageWorkspace, &[]);
    assert!(architect.contains("LEGACY FALLBACK OUTCOMES"));
    assert!(!architect.contains("WORKFLOW OUTCOMES"));
    assert!(architect.contains("`ready_code`"));
    assert!(architect.contains("`needs_design`"));
    assert!(architect.contains("`needs_breakdown`"));
}

#[test]
fn declared_outcomes_without_contracts_do_not_render_fallback_requirements() {
    let allowed = vec!["ship_it".to_string(), "hold_it".to_string()];
    let architect = system_prompt(Capability::TriageWorkspace, &allowed);
    assert!(architect.contains("WORKFLOW OUTCOMES"));
    assert!(architect.contains("- Verdict `ship_it`."));
    assert!(architect.contains("- Verdict `hold_it`."));
    assert!(!architect.contains("LEGACY FALLBACK OUTCOMES"));
    assert!(!architect.contains("ready_code"));
    assert!(!architect.contains("needs_design"));
    assert!(!architect.contains("needs_breakdown"));
    assert!(!architect.contains("child product(s)"));
    assert!(!architect.contains("authored `body`"));
    assert!(!architect.contains("pull-request `title`"));
}

#[test]
fn system_prompt_renders_only_declared_outcomes() {
    let allowed = vec!["ready_code".to_string(), "needs_design".to_string()];
    let architect = system_prompt(Capability::TriageWorkspace, &allowed);
    assert!(architect.contains("WORKFLOW OUTCOMES"));
    assert!(architect.contains("`ready_code`"));
    assert!(architect.contains("`needs_design`"));
    assert!(!architect.contains("`needs_breakdown`"));
    assert!(!architect.contains("LEGACY FALLBACK OUTCOMES"));
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
    assert!(prompt.contains("Verdict `validated` requires exactly 0 child product(s)"));
    assert!(prompt.contains("requires a non-blank pull-request `title`"));
    assert!(prompt.contains("requires a non-blank pull-request `body`"));
    assert!(prompt.contains("workflow metadata `target_branch`"));
}

#[test]
fn system_prompt_single_declared_outcome_is_the_only_verdict() {
    let allowed = vec!["ready_code".to_string()];
    let architect = system_prompt(Capability::TriageWorkspace, &allowed);
    assert!(architect.contains("WORKFLOW OUTCOMES"));
    assert!(architect.contains("- Verdict `ready_code`."));
    assert!(!architect.contains("needs_design"));
    assert!(!architect.contains("needs_breakdown"));
}

#[test]
fn system_prompt_engineer_keeps_head_path_with_declared_outcomes() {
    let allowed = vec!["external_blocker".to_string()];
    let engineer = system_prompt(Capability::CodingWorkspace, &allowed);
    assert!(engineer.contains("WORKFLOW OUTCOMES"));
    assert!(engineer.contains("no-verdict engineer success path remains available"));
    assert!(engineer.contains("- Verdict `external_blocker`."));
    assert!(!engineer.contains("needs_architect"));
    assert!(!engineer.contains("needs_human"));
}

#[test]
fn user_context_includes_work_item_and_guidance() {
    let context = parsed_fixture();
    let rendered = user_context(&context);
    assert!(rendered.contains("acme/service"));
    assert!(rendered.contains("dir: service/"));
    assert!(rendered.contains("manifest/repository access policy: writable"));
    assert!(rendered.contains("Role: engineer"));
    assert!(rendered.contains("Action: open_pr"));
    assert!(rendered.contains("Target: Issue { number: ItemNumber(7) }"));
    assert!(rendered.contains("base branch: main"));
    assert!(rendered.contains("work branch: agent/pr-for-code-7"));
    assert!(rendered.contains("Correlation key: pr-for-code-7"));
    assert!(rendered.contains("Checkout mode: writable"));
    assert!(rendered.contains("Effective checkout authority: writable"));
    assert!(rendered.contains("Edits are permitted only in repositories whose manifest/repository access policy is `writable`"));
    assert!(rendered.contains("Make a real product change."));
    assert!(rendered.contains("Use docs/product-change.md"));
    assert!(rendered.contains("No .temper-only diffs."));
    assert!(rendered.contains(r#"{"artifact":{"title":"Implement docs"}}"#));
    assert!(rendered.contains("Work item context (JSON):"));
}

#[test]
fn read_only_checkout_overrides_writable_manifest_policy() {
    for (role, checkout) in [
        ("architect", "read_only"),
        ("reviewer", "pull_request_read_only"),
    ] {
        let mut context = parsed_fixture();
        context.work_item.role = role.to_string();
        context.checkout = Some(checkout.to_string());
        context.guidance = WorkspaceGuidance::default();

        let rendered = user_context(&context);
        assert!(rendered.contains("manifest/repository access policy: writable"));
        assert!(rendered.contains(&format!(
            "Effective checkout authority: read-only (`{checkout}`)"
        )));
        assert!(rendered.contains("No repository may be modified"));
        assert!(rendered.contains(
            "overrides writable manifest/repository access policy and all branch or work-branch hints"
        ));
        assert!(!rendered.contains("Edit files"));
        assert!(!rendered.contains("Edits are permitted"));
    }
}

#[test]
fn writable_and_legacy_contexts_render_effective_authority() {
    let mut context = parsed_fixture();

    let writable = user_context(&context);
    assert!(writable.contains("Effective checkout authority: writable"));
    assert!(writable.contains("Edits are permitted only in repositories whose manifest/repository access policy is `writable`"));

    context.checkout = Some("pull_request_writable".to_string());
    let pull_request_writable = user_context(&context);
    assert!(pull_request_writable.contains("Effective checkout authority: pull-request writable"));
    assert!(pull_request_writable.contains("no other repository may be modified"));

    context.checkout = None;
    let legacy = user_context(&context);
    assert!(legacy.contains("Effective checkout authority: not supplied by this legacy context"));
    assert!(legacy.contains("branch hints remain the available legacy authority signals"));
    assert!(!legacy.contains("Checkout mode:"));
}

#[test]
fn multi_repo_preamble_obeys_checkout_authority() {
    let mut context = parsed_fixture();
    let mut dependency = context.repos[0].clone();
    dependency.id = "repo-2".to_string();
    dependency.name = "dependency".to_string();
    dependency.dir = "dependency".to_string();
    dependency.access = "read_only".to_string();
    dependency.branch_hint = None;
    context.repos.push(dependency);
    context.guidance = WorkspaceGuidance::default();

    let writable = user_context(&context);
    assert!(writable.contains("COORDINATED multi-repo workspace"));
    assert!(
        writable.contains(
            "Edit files only inside repositories whose manifest access policy is writable"
        )
    );
    assert!(writable.contains("manifest/repository access policy: read_only"));

    context.checkout = Some("read_only".to_string());
    let read_only = user_context(&context);
    assert!(read_only.contains(
        "do not modify any repository, including repositories whose manifest policy is writable"
    ));
    assert!(read_only.contains("No repository may be modified"));
    assert!(!read_only.contains("Edit files only"));
    assert!(!read_only.contains("edit files inside"));
}

#[test]
fn workflow_prompts_use_the_coordinating_artifact_as_primary() {
    let cases = [
        ("plan_feature", "architect", "issue", 101, "Feature"),
        ("decompose_plan", "architect", "issue", 102, "Plan"),
        ("validate_plan", "tester", "issue", 102, "Plan"),
        ("open_pr", "engineer", "issue", 103, "Code child"),
        (
            "address_implementation_ci_failure",
            "engineer",
            "pull_request",
            104,
            "Implementation PR",
        ),
        (
            "review_pr",
            "reviewer",
            "pull_request",
            104,
            "Implementation PR",
        ),
    ];

    for (action, role, artifact_type, number, title) in cases {
        let mut context = parsed_fixture();
        context.action = action.to_string();
        context.work_item.role = role.to_string();
        context.artifact_context = Some(
            serde_json::from_value(serde_json::json!({
                "version": 1,
                "repository": {"id":"repo-1", "path":"acme/service"},
                "artifact_type": artifact_type,
                "primary": snapshot(artifact_type, number, title, "coordinating body", &["ready"], if artifact_type == "issue" { "code" } else { "implementation_pr" }),
                "lineage": [snapshot("issue", 100, "Mandatory ancestor", "ancestor body", &["feature"], "feature")],
                "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
            }))
            .expect("workflow bundle parses"),
        );

        let rendered = user_context(&context);
        let primary = artifact_section(&rendered, "Primary artifact:", "Mandatory lineage:");
        let lineage = artifact_section(&rendered, "Mandatory lineage:", "Validation summaries:");
        assert!(
            primary.contains(&format!("{title} [open]")),
            "action {action} primary section: {primary}"
        );
        assert!(primary.contains("coordinating body"), "action {action}");
        assert!(!primary.contains("Mandatory ancestor"), "action {action}");
        assert!(lineage.contains("Mandatory ancestor"), "action {action}");
        assert!(!lineage.contains(title), "action {action}");
        assert!(rendered.contains(&format!("Role: {role}")));
    }
}

#[test]
fn artifact_bundle_renders_only_explicit_members_in_each_section() {
    let mut context = parsed_fixture();
    context.action = "validate_plan".to_string();
    context.work_item.role = "tester".to_string();
    context.artifact_context = Some(serde_json::from_value(serde_json::json!({
        "version": 1,
        "repository": {"id":"repo-1", "path":"acme/service"},
        "artifact_type": "issue",
        "primary": snapshot("issue", 7, "Validation plan", "plan body", &["needs-validation", "plan"], "plan"),
        "lineage": [
            snapshot("issue", 1, "Feature root", "feature body", &["feature"], "feature"),
            snapshot("issue", 3, "Design parent", "design body", &["design"], "design")
        ],
        "validation_scope": [
            summary("issue", 8, "Code child", &["code", "implemented"], "code", "dependency", "issue", 7),
            summary("pull_request", 9, "Implementation PR", &["implementation"], "implementation_pr", "related", "issue", 8)
        ],
        "optional_references": [
            summary("issue", 11, "Markdown reference", &["docs"], "reference", "related", "issue", 3)
        ],
        "diagnostics":[{"code":"content_truncated","message":"body bounded","source":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":7}}],
        "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":true}
    })).expect("bundle parses"));

    let rendered = user_context(&context);
    let primary = artifact_section(&rendered, "Primary artifact:", "Mandatory lineage:");
    assert_eq!(
        primary,
        "- issue acme/service#7 — Validation plan [open] labels=needs-validation, plan\n  Body:\nplan body\n  Workflow context:\n    kind: plan"
    );

    let lineage = artifact_section(&rendered, "Mandatory lineage:", "Validation summaries:");
    assert_eq!(
        lineage,
        "- issue acme/service#1 — Feature root [open] labels=feature\n  Body:\nfeature body\n  Workflow context:\n    kind: feature\n- issue acme/service#3 — Design parent [open] labels=design\n  Body:\ndesign body\n  Workflow context:\n    kind: design"
    );

    let validation = artifact_section(
        &rendered,
        "Validation summaries:",
        "Optional body-omitted references:",
    );
    assert_eq!(
        validation,
        "- issue acme/service#8 — Code child [open] kind=code labels=code, implemented relation=dependency source=issue acme/service#7\n- pull request acme/service#9 — Implementation PR [open] kind=implementation_pr labels=implementation relation=related source=issue acme/service#8"
    );

    let optional = artifact_section(
        &rendered,
        "Optional body-omitted references:",
        "Diagnostics and truncation:",
    );
    assert_eq!(
        optional,
        "- issue acme/service#11 — Markdown reference [open] kind=reference labels=docs relation=related source=issue acme/service#3"
    );
    assert!(!optional.contains("Code child"));
    assert!(!optional.contains("Implementation PR"));

    let diagnostics = rendered
        .split_once("Diagnostics and truncation:")
        .expect("diagnostics heading")
        .1
        .trim();
    assert!(diagnostics.contains("content_truncated (issue acme/service#7): body bounded"));
    assert!(diagnostics.contains("content_truncated=true"));
    assert!(!rendered.contains("Forge context follow-up:"));
    assert!(!rendered.contains("Work item context (JSON):"));
}

fn snapshot(
    artifact_type: &str,
    number: u64,
    title: &str,
    body: &str,
    labels: &[&str],
    workflow_kind: &str,
) -> serde_json::Value {
    serde_json::json!({
        "artifact": {
            "repository": {"id":"repo-1", "path":"acme/service"},
            "artifact_type": artifact_type,
            "number": number
        },
        "title": title,
        "body": body,
        "labels": labels,
        "state": "open",
        "workflow_kind": workflow_kind
    })
}

#[allow(clippy::too_many_arguments)]
fn summary(
    artifact_type: &str,
    number: u64,
    title: &str,
    labels: &[&str],
    workflow_kind: &str,
    relation_type: &str,
    source_type: &str,
    source_number: u64,
) -> serde_json::Value {
    serde_json::json!({
        "artifact": {
            "repository": {"id":"repo-1", "path":"acme/service"},
            "artifact_type": artifact_type,
            "number": number
        },
        "title": title,
        "labels": labels,
        "state": "open",
        "workflow_kind": workflow_kind,
        "relation_type": relation_type,
        "source": {
            "repository": {"id":"repo-1", "path":"acme/service"},
            "artifact_type": source_type,
            "number": source_number
        }
    })
}

fn artifact_section<'a>(rendered: &'a str, heading: &str, next_heading: &str) -> &'a str {
    rendered
        .split_once(heading)
        .unwrap_or_else(|| panic!("missing heading {heading}"))
        .1
        .split_once(next_heading)
        .unwrap_or_else(|| panic!("missing following heading {next_heading}"))
        .0
        .trim()
}
