//! Authored artifact-body and compact workflow-context prompt rendering.

use super::common::*;
use crate::coding_agent::*;

#[test]
fn authored_lineage_and_compact_workflow_context_render_deterministically() {
    const FEATURE_BODY: &str = "## Objective\nFEATURE_OBJECTIVE_SENTINEL\n## Constraint\nFEATURE_CONSTRAINT_SENTINEL\n## Tests\nFEATURE_TEST_SENTINEL\n## Acceptance\nFEATURE_ACCEPTANCE_SENTINEL";
    const PLAN_BODY: &str = "## Objective\nPLAN_OBJECTIVE_SENTINEL\n## Constraint\nPLAN_CONSTRAINT_SENTINEL\n## Tests\nPLAN_TEST_SENTINEL\n## Acceptance\nPLAN_ACCEPTANCE_SENTINEL";
    const CODE_BODY: &str = "## Objective\nCODE_OBJECTIVE_SENTINEL\n## Constraint\nCODE_CONSTRAINT_SENTINEL\n## Tests\nCODE_TEST_SENTINEL\n## Acceptance\nCODE_ACCEPTANCE_SENTINEL";

    let context = projected_lineage_context("PRIVATE_BOOKKEEPING_A", false);
    let rendered = user_context(&context);

    for body in [FEATURE_BODY, PLAN_BODY, CODE_BODY] {
        assert!(rendered.contains(body), "authored body changed: {body}");
    }
    for forbidden in [
        "body_hex",
        "create_issue_intents",
        "completion",
        "lease",
        "wired",
        "NESTED_CHILD_BODY_SENTINEL",
        "PRIVATE_BOOKKEEPING_A",
        "legacy-conflicting-code",
        "legacy-conflicting-plan",
    ] {
        assert!(!rendered.contains(forbidden), "leaked {forbidden}");
    }

    assert!(rendered.contains("    kind: feature"));
    assert!(rendered.contains("    kind: plan"));
    assert!(rendered.contains("    kind: code"));
    assert!(rendered.contains("    parents: repo-1#10, repo-2#2"));
    assert!(rendered.contains("    dependencies: repo-1#30"));
    assert!(rendered.contains("    target branch: feature/authored-context"));
    assert!(rendered.contains("    correlation key: correlation-plan"));
    assert!(rendered.contains("      - repo-1#20 — Plan child [open]"));
    assert!(rendered.contains("      - repo-1#30 — Code child [open]"));
    assert!(rendered.contains("      - repo-3#42 — A canonical title [open]"));

    let validation = artifact_section(
        &rendered,
        "Validation summaries:",
        "Optional body-omitted references:",
    );
    assert_eq!(validation.lines().count(), 2);
    assert!(validation.contains("Validation dependency"));
    assert!(validation.contains("Implementation PR"));
    let optional = artifact_section(
        &rendered,
        "Optional body-omitted references:",
        "Diagnostics and truncation:",
    );
    assert!(optional.contains("Incidental reference"));
    assert!(!optional.contains("OPTIONAL_BODY_SENTINEL"));

    for _ in 0..8 {
        assert_eq!(user_context(&context), rendered);
    }
    let bookkeeping_changed =
        user_context(&projected_lineage_context("PRIVATE_BOOKKEEPING_B", false));
    assert_eq!(bookkeeping_changed, rendered);

    let with_new_identity = user_context(&projected_lineage_context("PRIVATE_BOOKKEEPING_B", true));
    let added_identity = "      - repo-z#99 — Newly persisted child [closed]\n";
    assert!(with_new_identity.contains(added_identity.trim_end()));
    assert_eq!(with_new_identity.replacen(added_identity, "", 1), rendered);
}

#[test]
fn compact_kind_overrides_legacy_and_legacy_kind_falls_back_once() {
    let mut context = parsed_fixture();
    context.guidance = WorkspaceGuidance::default();
    let mut primary = snapshot(
        "issue",
        7,
        "Projected kind",
        "projected body",
        &["code"],
        "legacy-conflict",
    );
    primary["workflow"] = serde_json::json!({"kind":"projected-code"});
    context.artifact_context = Some(
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "repository": {"id":"repo-1", "path":"acme/service"},
            "artifact_type": "issue",
            "primary": primary,
            "lineage": [snapshot(
                "issue",
                1,
                "Legacy kind",
                "legacy body",
                &["feature"],
                "legacy-feature"
            )],
            "truncation": {
                "depth_exceeded": false,
                "count_exceeded": false,
                "content_truncated": false
            }
        }))
        .expect("kind fixture parses"),
    );

    let rendered = user_context(&context);
    let primary = artifact_section(&rendered, "Primary artifact:", "Mandatory lineage:");
    assert_eq!(primary.matches("kind: projected-code").count(), 1);
    assert!(!primary.contains("legacy-conflict"));
    assert!(!primary.contains("kind="));
    let lineage = artifact_section(&rendered, "Mandatory lineage:", "Validation summaries:");
    assert_eq!(lineage.matches("kind: legacy-feature").count(), 1);
    assert!(!lineage.contains("kind="));
}

#[test]
fn large_lineage_renders_decision_dense_projection_without_trimming_primary() {
    let mut context = parsed_fixture();
    context.guidance = WorkspaceGuidance::default();
    let repeated = "repeated planning prose ".repeat(90);
    let ancestor_body = format!(
        "## Objective\n{repeated}\n## Constraints\nKEEP_CONSTRAINT\n## Acceptance\n{repeated}\n## Architecture\nKEEP_ARCHITECTURE\n## Test mapping\n{repeated}\n## Non-goals\nKEEP_NON_GOAL"
    );
    context.artifact_context = Some(
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "repository": {"id":"repo-1", "path":"acme/service"},
            "artifact_type":"issue",
            "primary": snapshot("issue", 30, "Code", "PRIMARY_BODY_UNCHANGED", &["code"], "code"),
            "lineage": [
                snapshot("issue", 10, "Feature", &ancestor_body, &["feature"], "feature"),
                snapshot("issue", 20, "Plan", &ancestor_body, &["plan"], "plan")
            ],
            "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
        }))
        .unwrap(),
    );

    let rendered = user_context(&context);
    assert!(rendered.contains("PRIMARY_BODY_UNCHANGED"));
    assert!(rendered.contains("Projection: decision-dense"));
    assert_eq!(rendered.matches("KEEP_CONSTRAINT").count(), 2);
    assert_eq!(rendered.matches("KEEP_ARCHITECTURE").count(), 2);
    assert_eq!(rendered.matches("KEEP_NON_GOAL").count(), 2);
    assert!(!rendered.contains("## Acceptance"));
    assert!(!rendered.contains("## Test mapping"));
}

fn projected_lineage_context(bookkeeping: &str, expose_new_child: bool) -> WorkspaceContext {
    let mut context = parsed_fixture();
    context.guidance = WorkspaceGuidance::default();
    let mut primary_children = vec![
        serde_json::json!({
            "repository_id": "repo-3",
            "number": 42,
            "title": "Z duplicate title",
            "state": "open"
        }),
        serde_json::json!({
            "repository_id": "repo-3",
            "number": 42,
            "title": "A canonical title"
        }),
    ];
    if expose_new_child {
        primary_children.push(serde_json::json!({
            "repository_id": "repo-z",
            "number": 99,
            "title": "Newly persisted child",
            "state": "closed"
        }));
    }

    context.artifact_context = Some(
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "repository": {"id":"repo-1", "path":"acme/service"},
            "artifact_type": "issue",
            "primary": {
                "artifact": {
                    "repository": {"id":"repo-1", "path":"acme/service"},
                    "artifact_type": "issue",
                    "number": 30
                },
                "title": "Code artifact",
                "body": "## Objective\nCODE_OBJECTIVE_SENTINEL\n## Constraint\nCODE_CONSTRAINT_SENTINEL\n## Tests\nCODE_TEST_SENTINEL\n## Acceptance\nCODE_ACCEPTANCE_SENTINEL",
                "labels": ["code", "ready"],
                "state": "open",
                "workflow_kind": "legacy-conflicting-code",
                "workflow": {
                    "kind": "code",
                    "parents": [
                        {"repository_id":"repo-1", "number":20},
                        {"repository_id":"repo-1", "number":20}
                    ],
                    "dependencies": [
                        {"repository_id":"repo-2", "number":88},
                        {"repository_id":"repo-2", "number":88}
                    ],
                    "target_branch": "feature/authored-context",
                    "correlation_key": "correlation-code",
                    "children": primary_children
                },
                "create_issue_intents": {
                    "body_hex": bookkeeping,
                    "completion": bookkeeping,
                    "wired": true,
                    "nested_body": "NESTED_CHILD_BODY_SENTINEL"
                }
            },
            "lineage": [
                {
                    "artifact": {
                        "repository": {"id":"repo-1", "path":"acme/service"},
                        "artifact_type": "issue",
                        "number": 10
                    },
                    "title": "Feature artifact",
                    "body": "## Objective\nFEATURE_OBJECTIVE_SENTINEL\n## Constraint\nFEATURE_CONSTRAINT_SENTINEL\n## Tests\nFEATURE_TEST_SENTINEL\n## Acceptance\nFEATURE_ACCEPTANCE_SENTINEL",
                    "labels": ["feature"],
                    "state": "open",
                    "workflow_kind": "feature",
                    "workflow": {
                        "kind": "feature",
                        "children": [
                            {
                                "repository_id":"repo-1",
                                "number":20,
                                "title":"Plan child",
                                "state":"open"
                            },
                            {
                                "repository_id":"repo-1",
                                "number":20,
                                "title":"Plan child",
                                "state":"open"
                            }
                        ]
                    },
                    "lease": bookkeeping
                },
                {
                    "artifact": {
                        "repository": {"id":"repo-1", "path":"acme/service"},
                        "artifact_type": "issue",
                        "number": 20
                    },
                    "title": "Plan artifact",
                    "body": "## Objective\nPLAN_OBJECTIVE_SENTINEL\n## Constraint\nPLAN_CONSTRAINT_SENTINEL\n## Tests\nPLAN_TEST_SENTINEL\n## Acceptance\nPLAN_ACCEPTANCE_SENTINEL",
                    "labels": ["plan"],
                    "state": "open",
                    "workflow_kind": "legacy-conflicting-plan",
                    "workflow": {
                        "kind": "plan",
                        "parents": [
                            {"repository_id":"repo-2", "number":2},
                            {"repository_id":"repo-1", "number":10},
                            {"repository_id":"repo-2", "number":2}
                        ],
                        "dependencies": [
                            {"repository_id":"repo-1", "number":30},
                            {"repository_id":"repo-1", "number":30}
                        ],
                        "target_branch": "feature/authored-context",
                        "correlation_key": "correlation-plan",
                        "children": [{
                            "repository_id":"repo-1",
                            "number":30,
                            "title":"Code child",
                            "state":"open"
                        }]
                    },
                    "completion": bookkeeping
                }
            ],
            "validation_scope": [
                summary(
                    "issue", 31, "Validation dependency", &["code"], "code",
                    "dependency", "issue", 30
                ),
                summary(
                    "pull_request", 32, "Implementation PR", &["implementation"],
                    "implementation_pr", "related", "issue", 31
                )
            ],
            "optional_references": [{
                "artifact": {
                    "repository": {"id":"repo-1", "path":"acme/service"},
                    "artifact_type": "issue",
                    "number": 40
                },
                "title": "Incidental reference",
                "body": "OPTIONAL_BODY_SENTINEL",
                "labels": ["docs"],
                "state": "open",
                "workflow_kind": "reference",
                "relation_type": "related",
                "source": {
                    "repository": {"id":"repo-1", "path":"acme/service"},
                    "artifact_type": "issue",
                    "number": 10
                }
            }],
            "truncation": {
                "depth_exceeded": false,
                "count_exceeded": false,
                "content_truncated": false
            }
        }))
        .expect("projected lineage fixture parses"),
    );
    context
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
