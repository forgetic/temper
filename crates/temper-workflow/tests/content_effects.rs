//! Content-bearing single-artifact effect tests (ADR 0022 §C).
//!
//! `set_body` writes an agent-authored body onto the current artifact and
//! `attach_review` submits a native pull-request review carrying an authored
//! body. Both read their content from the workspace work product through the
//! keyed runtime-input seam on [`ExecutionContext`], exactly as
//! `create_pull_request` reads its head — so these tests bind the content the
//! way the runner binds a workspace work product, then assert the effect
//! applied idempotently across retries.

mod support;

use support::{block_on, create_issue, create_pr, issue_body, new_repo, TestRoot};
use temper_forge::{Forge, ReviewDecision};
use temper_workflow::{
    ArtifactSource, Effect, ExecutionContext, ExecutionError, RawEffect, RawWorkflowSpec, RoleId,
    TransitionId, ValidatedWorkflow, WorkflowEffect,
};

const SET_BODY_WORKFLOW: &str = r#"{
    "name": "set-body",
    "roles": [{"id": "architect"}],
    "labels": [{"id": "code"}, {"id": "ready"}],
    "artifact_kinds": [
        {"id": "code", "target": "issue", "identifying_labels": ["code"]}
    ],
    "transitions": [{"id": "rewrite_intake", "artifact": "code", "roles": ["architect"], "effects": [
        {"kind": "set_body", "correlation_key": "body-code-1"},
        {"kind": "add_label", "label": "ready"}
    ]}]
}"#;

const ATTACH_REVIEW_WORKFLOW: &str = r#"{
    "name": "attach-review",
    "roles": [{"id": "reviewer"}],
    "labels": [{"id": "implementation"}, {"id": "changes-requested"}],
    "artifact_kinds": [
        {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
    ],
    "transitions": [{"id": "request_changes", "artifact": "implementation_pr", "roles": ["reviewer"], "effects": [
        {"kind": "attach_review", "decision": "changes_requested", "correlation_key": "review-pr-1"},
        {"kind": "add_label", "label": "changes-requested"}
    ]}]
}"#;

const SET_BODY_ON_PR_WORKFLOW: &str = r#"{
    "name": "set-body-pr",
    "roles": [{"id": "architect"}],
    "labels": [{"id": "implementation"}],
    "artifact_kinds": [
        {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
    ],
    "transitions": [{"id": "rewrite_pr", "artifact": "implementation_pr", "roles": ["architect"], "effects": [
        {"kind": "set_body", "correlation_key": "body-pr-1"}
    ]}]
}"#;

const ATTACH_REVIEW_ON_ISSUE_WORKFLOW: &str = r#"{
    "name": "attach-review-issue",
    "roles": [{"id": "reviewer"}],
    "labels": [{"id": "code"}],
    "artifact_kinds": [
        {"id": "code", "target": "issue", "identifying_labels": ["code"]}
    ],
    "transitions": [{"id": "bad_review", "artifact": "code", "roles": ["reviewer"], "effects": [
        {"kind": "attach_review", "decision": "commented", "correlation_key": "review-issue-1"}
    ]}]
}"#;

const DYNAMIC_SET_BODY_WORKFLOW: &str = r#"{
    "name": "dynamic-set-body",
    "roles": [{"id": "architect"}],
    "labels": [{"id": "code"}],
    "artifact_kinds": [
        {"id": "code", "target": "issue", "identifying_labels": ["code"]}
    ],
    "transitions": [{"id": "rewrite_intake", "artifact": "code", "roles": ["architect"], "effects": [
        {"kind": "set_body"}
    ]}]
}"#;

fn validate(spec_json: &str) -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(spec_json).expect("json parses");
    spec.validate().expect("workflow validates")
}

#[test]
fn content_effects_deserialize_with_and_without_a_correlation_key() {
    // `set_body` with an explicit key.
    let set_body: RawEffect =
        serde_json::from_str(r#"{"kind": "set_body", "correlation_key": "k1"}"#).unwrap();
    assert!(matches!(
        set_body,
        RawEffect::SetBody { correlation_key: Some(ref k) } if k == "k1"
    ));
    // `set_body` with the key defaulted away.
    let set_body_default: RawEffect = serde_json::from_str(r#"{"kind": "set_body"}"#).unwrap();
    assert!(matches!(
        set_body_default,
        RawEffect::SetBody {
            correlation_key: None
        }
    ));
    // `attach_review` carries the portable decision; the key defaults.
    let attach_review: RawEffect =
        serde_json::from_str(r#"{"kind": "attach_review", "decision": "changes_requested"}"#)
            .unwrap();
    assert!(matches!(
        attach_review,
        RawEffect::AttachReview {
            decision: ReviewDecision::ChangesRequested,
            correlation_key: None,
        }
    ));
}

#[test]
fn content_effects_validate_and_compile_into_tool_manifests() {
    // The runtime seam validates: a transition declaring set_body / attach_review
    // compiles into a tool manifest carrying the typed effects, so a
    // workspace-backed action can expose them.
    let workflow = validate(SET_BODY_WORKFLOW);
    let compiled = workflow.compile();
    let role = compiled
        .role(&RoleId::new("architect"))
        .expect("role compiles");
    let tool = role
        .tools
        .iter()
        .find(|tool| tool.transition == TransitionId::new("rewrite_intake"))
        .expect("rewrite_intake tool compiles");
    assert!(tool.effects.iter().any(|effect| matches!(
        effect,
        Effect::SetBody { correlation_key: Some(key) } if key == "body-code-1"
    )));

    let review_workflow = validate(ATTACH_REVIEW_WORKFLOW);
    let review_compiled = review_workflow.compile();
    let review_role = review_compiled
        .role(&RoleId::new("reviewer"))
        .expect("role compiles");
    let review_tool = review_role
        .tools
        .iter()
        .find(|tool| tool.transition == TransitionId::new("request_changes"))
        .expect("request_changes tool compiles");
    assert!(review_tool.effects.iter().any(|effect| matches!(
        effect,
        Effect::AttachReview {
            decision: ReviewDecision::ChangesRequested,
            correlation_key: Some(key),
        } if key == "review-pr-1"
    )));
}

const AUTHORED_BODY: &str =
    "# Rewritten intake\n\nCrisp, implementable spec authored by the architect workspace.";
const REVIEW_BODY: &str = "Please address the null check on line 42 before this can land.";

#[test]
fn set_body_writes_the_authored_body_onto_an_issue() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(SET_BODY_WORKFLOW);
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code"], "raw human intake");
    let context = ExecutionContext::new().with_set_body_at(
        TransitionId::new("rewrite_intake"),
        0,
        AUTHORED_BODY,
    );
    let executor = workflow.executor_with_context(&forge, context);

    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: issue },
        &TransitionId::new("rewrite_intake"),
        &RoleId::new("architect"),
    ))
    .expect("set_body executes");
    assert!(report.applied.contains(&WorkflowEffect::SetBody {
        correlation_key: Some("body-code-1".into()),
    }));

    assert_eq!(issue_body(&forge, &repo, issue), AUTHORED_BODY);
}

#[test]
fn set_body_is_idempotent_across_retries() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(DYNAMIC_SET_BODY_WORKFLOW);
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code"], "raw human intake");
    let context = ExecutionContext::new()
        .with_set_body_at(TransitionId::new("rewrite_intake"), 0, AUTHORED_BODY)
        .with_set_body_correlation_key_at(TransitionId::new("rewrite_intake"), 0, "body-code-9");
    let executor = workflow.executor_with_context(&forge, context);

    let target = ArtifactSource::Issue { number: issue };
    let transition = TransitionId::new("rewrite_intake");
    let role = RoleId::new("architect");

    block_on(executor.execute(&repo, target, &transition, &role)).expect("first set_body executes");
    assert_eq!(issue_body(&forge, &repo, issue), AUTHORED_BODY);
    // Re-applying overwrites with the same authored content: still idempotent.
    block_on(executor.execute(&repo, target, &transition, &role)).expect("retry set_body executes");
    assert_eq!(issue_body(&forge, &repo, issue), AUTHORED_BODY);
}

#[test]
fn set_body_without_bound_body_fails_before_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(SET_BODY_WORKFLOW);
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code"], "raw human intake");
    // No body bound in the context: the seam is empty.
    let executor = workflow.executor(&forge);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: issue },
        &TransitionId::new("rewrite_intake"),
        &RoleId::new("architect"),
    ))
    .expect_err("set_body with no bound body is rejected");
    assert!(matches!(
        error,
        ExecutionError::UnresolvedSetBody {
            effect_index: 0,
            ..
        }
    ));
    // The label effect must NOT have applied: validation precedes mutation.
    let labels = {
        let mut labels = block_on(forge.get_issue_by_number(&repo, issue))
            .unwrap()
            .unwrap()
            .labels;
        labels.sort();
        labels
    };
    assert_eq!(labels, vec!["code".to_string()]);
    assert_eq!(issue_body(&forge, &repo, issue), "raw human intake");
}

#[test]
fn set_body_writes_the_authored_body_onto_a_pull_request() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(SET_BODY_ON_PR_WORKFLOW);
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "original PR body");
    let context =
        ExecutionContext::new().with_set_body_at(TransitionId::new("rewrite_pr"), 0, AUTHORED_BODY);
    let executor = workflow.executor_with_context(&forge, context);

    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("rewrite_pr"),
        &RoleId::new("architect"),
    ))
    .expect("set_body on a PR executes");

    let body = block_on(forge.get_pull_request_by_number(&repo, number))
        .unwrap()
        .unwrap()
        .body;
    assert_eq!(body, AUTHORED_BODY);
}

#[test]
fn attach_review_submits_a_native_review_with_the_authored_body() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(ATTACH_REVIEW_WORKFLOW);
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "");
    let context = ExecutionContext::new().with_attach_review_at(
        TransitionId::new("request_changes"),
        0,
        REVIEW_BODY,
    );
    let executor = workflow.executor_with_context(&forge, context);

    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("request_changes"),
        &RoleId::new("reviewer"),
    ))
    .expect("attach_review executes");

    let pull_request = block_on(forge.get_pull_request_by_number(&repo, number))
        .unwrap()
        .unwrap();
    let reviews =
        block_on(forge.list_pull_request_reviews(&pull_request.id)).expect("reviews list");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].decision, ReviewDecision::ChangesRequested);
    let review_body = reviews[0].body.as_deref().expect("review carries a body");
    assert!(
        review_body.contains(REVIEW_BODY),
        "review body should carry the authored prose, got: {review_body}"
    );
    // The co-declared label effect applied through the same transition.
    assert!(pull_request
        .labels
        .iter()
        .any(|label| label == "changes-requested"));
}

#[test]
fn attach_review_is_idempotent_across_retries() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(ATTACH_REVIEW_WORKFLOW);
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "");
    let context = ExecutionContext::new().with_attach_review_at(
        TransitionId::new("request_changes"),
        0,
        REVIEW_BODY,
    );
    let executor = workflow.executor_with_context(&forge, context);

    let target = ArtifactSource::PullRequest { number };
    let transition = TransitionId::new("request_changes");
    let role = RoleId::new("reviewer");

    block_on(executor.execute(&repo, target, &transition, &role))
        .expect("first attach_review executes");
    // The label flip already happened; a retry re-plans against fresh state.
    // The review must not be duplicated even though the transition re-runs.
    let _ = block_on(executor.execute(&repo, target, &transition, &role));

    let pull_request = block_on(forge.get_pull_request_by_number(&repo, number))
        .unwrap()
        .unwrap();
    let reviews =
        block_on(forge.list_pull_request_reviews(&pull_request.id)).expect("reviews list");
    assert_eq!(
        reviews.len(),
        1,
        "attach_review must submit at most one review per correlation key"
    );
}

#[test]
fn attach_review_on_an_issue_fails_before_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(ATTACH_REVIEW_ON_ISSUE_WORKFLOW);
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code"], "an issue, not a PR");
    let context = ExecutionContext::new().with_attach_review_at(
        TransitionId::new("bad_review"),
        0,
        REVIEW_BODY,
    );
    let executor = workflow.executor_with_context(&forge, context);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: issue },
        &TransitionId::new("bad_review"),
        &RoleId::new("reviewer"),
    ))
    .expect_err("attach_review is rejected on a non-PR target");
    assert!(matches!(
        error,
        ExecutionError::UnsupportedEffect {
            effect: WorkflowEffect::AttachReview { .. }
        }
    ));
}

#[test]
fn attach_review_accepts_a_runtime_correlation_key() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(
        r#"{
            "name": "dynamic-attach-review",
            "roles": [{"id": "reviewer"}],
            "labels": [{"id": "implementation"}],
            "artifact_kinds": [
                {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
            ],
            "transitions": [{"id": "request_changes", "artifact": "implementation_pr", "roles": ["reviewer"], "effects": [
                {"kind": "attach_review", "decision": "changes_requested"}
            ]}]
        }"#,
    );
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "");
    let context = ExecutionContext::new()
        .with_attach_review_at(TransitionId::new("request_changes"), 0, REVIEW_BODY)
        .with_attach_review_correlation_key_at(
            TransitionId::new("request_changes"),
            0,
            "review-pr-77",
        );
    let executor = workflow.executor_with_context(&forge, context);

    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("request_changes"),
        &RoleId::new("reviewer"),
    ))
    .expect("attach_review executes with a runtime correlation key");

    let pull_request = block_on(forge.get_pull_request_by_number(&repo, number))
        .unwrap()
        .unwrap();
    let reviews =
        block_on(forge.list_pull_request_reviews(&pull_request.id)).expect("reviews list");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].decision, ReviewDecision::ChangesRequested);
}
