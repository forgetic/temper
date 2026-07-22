//! Target-branch workflow policy parsing, validation, and compilation tests.

use temper_workflow::{
    Diagnostic, Effect, RawEffect, RawWorkflowSpec, RoleId, TargetBranchPolicy, TransitionId,
};

const PLAN_CENTRIC_WORKFLOW: &str =
    include_str!("../../../scenarios/plan-centric-feature-branch/config/workflow.json");

fn effect<'a>(
    workflow: &'a temper_workflow::CompiledWorkflow,
    transition: &str,
    predicate: impl Fn(&Effect) -> bool,
) -> &'a Effect {
    workflow
        .transitions()
        .iter()
        .find(|candidate| candidate.id == TransitionId::new(transition))
        .and_then(|transition| transition.effects.iter().find(|effect| predicate(effect)))
        .unwrap_or_else(|| panic!("expected effect on transition `{transition}`"))
}

#[test]
fn target_branch_policies_parse_and_serialize_as_typed_tokens() {
    let cases = [
        (
            r#"{"kind":"create_issues","target_branch_policy":"derived_feature_branch"}"#,
            TargetBranchPolicy::DerivedFeatureBranch,
        ),
        (
            r#"{"kind":"create_issues","target_branch_policy":"inherit"}"#,
            TargetBranchPolicy::Inherit,
        ),
        (
            r#"{"kind":"create_pull_request","target_branch_policy":"non_default"}"#,
            TargetBranchPolicy::NonDefault,
        ),
        (
            r#"{"kind":"create_pull_request","target_branch_policy":"repository_default"}"#,
            TargetBranchPolicy::RepositoryDefault,
        ),
    ];

    for (json, expected) in cases {
        let raw: RawEffect = serde_json::from_str(json).expect("typed policy parses");
        let actual = match &raw {
            RawEffect::CreateIssues {
                target_branch_policy,
                ..
            }
            | RawEffect::CreatePullRequest {
                target_branch_policy,
                ..
            } => *target_branch_policy,
            _ => panic!("branch policy only appears on branch effects"),
        };
        assert_eq!(actual, Some(expected));
        assert_eq!(
            serde_json::to_value(raw).expect("effect serializes")["target_branch_policy"],
            expected.as_str()
        );
    }
}

#[test]
fn omitted_policy_preserves_legacy_serialization_without_default_intent() {
    for json in [
        r#"{"kind":"create_issues"}"#,
        r#"{"kind":"create_pull_request"}"#,
    ] {
        let raw: RawEffect = serde_json::from_str(json).expect("legacy effect parses");
        let policy = match &raw {
            RawEffect::CreateIssues {
                target_branch_policy,
                ..
            }
            | RawEffect::CreatePullRequest {
                target_branch_policy,
                ..
            } => target_branch_policy,
            _ => unreachable!(),
        };
        assert_eq!(*policy, None);
        let serialized = serde_json::to_value(raw).expect("legacy effect serializes");
        assert!(serialized.get("target_branch_policy").is_none());
    }

    let explicit: RawEffect = serde_json::from_str(
        r#"{"kind":"create_pull_request","target_branch_policy":"repository_default"}"#,
    )
    .expect("explicit default policy parses");
    assert!(matches!(
        explicit,
        RawEffect::CreatePullRequest {
            target_branch_policy: Some(TargetBranchPolicy::RepositoryDefault),
            ..
        }
    ));
}

#[test]
fn unsupported_policy_effect_combinations_are_diagnosed_together() {
    let spec: RawWorkflowSpec = serde_json::from_str(
        r#"{
          "name": "invalid-branch-policies",
          "roles": [{"id": "architect"}],
          "labels": [{"id": "feature"}],
          "artifact_kinds": [
            {"id": "feature", "target": "issue", "identifying_labels": ["feature"]}
          ],
          "transitions": [{
            "id": "invalid",
            "artifact": "feature",
            "roles": ["architect"],
            "effects": [
              {"kind": "create_issues", "target_branch_policy": "non_default"},
              {"kind": "create_pull_request", "target_branch_policy": "inherit"},
              {"kind": "create_pull_request", "target_branch_policy": "derived_feature_branch"}
            ]
          }]
        }"#,
    )
    .expect("policy vocabulary parses before semantic validation");

    let errors = spec.validate().expect_err("unsupported combinations fail");
    for (effect, policy) in [
        ("create_issues", TargetBranchPolicy::NonDefault),
        ("create_pull_request", TargetBranchPolicy::Inherit),
        (
            "create_pull_request",
            TargetBranchPolicy::DerivedFeatureBranch,
        ),
    ] {
        assert!(
            errors
                .diagnostics()
                .contains(&Diagnostic::UnsupportedTargetBranchPolicy {
                    transition: "invalid".to_string(),
                    effect: effect.to_string(),
                    policy,
                })
        );
    }
}

#[test]
fn plan_centric_feature_branch_policies_validate_and_compile() {
    let spec: RawWorkflowSpec =
        serde_json::from_str(PLAN_CENTRIC_WORKFLOW).expect("plan-centric scenario workflow parses");
    let compiled = spec
        .validate()
        .expect("plan-centric scenario workflow validates")
        .compile();

    let issue_policy = |transition: &str| {
        effect(&compiled, transition, |effect| {
            matches!(effect, Effect::CreateIssues { .. })
        })
    };
    assert!(matches!(
        issue_policy("feature_to_plan"),
        Effect::CreateIssues {
            target_branch_policy: Some(TargetBranchPolicy::DerivedFeatureBranch),
            ..
        }
    ));
    let serialized =
        serde_json::to_string(issue_policy("feature_to_plan")).expect("compiled effect serializes");
    assert!(serialized.contains(r#""target_branch_policy":"derived_feature_branch""#));
    for transition in ["plan_children_created", "plan_validation_needs_followup"] {
        assert!(matches!(
            issue_policy(transition),
            Effect::CreateIssues {
                target_branch_policy: Some(TargetBranchPolicy::Inherit),
                ..
            }
        ));
    }

    for transition in ["open_pr", "plan_validated_create_landing"] {
        assert!(matches!(
            effect(&compiled, transition, |effect| matches!(
                effect,
                Effect::CreatePullRequest { .. }
            )),
            Effect::CreatePullRequest {
                target_branch_policy: Some(TargetBranchPolicy::NonDefault),
                ..
            }
        ));
    }
}

#[test]
fn repository_default_policy_compiles_as_explicit_same_branch_intent() {
    let spec: RawWorkflowSpec = serde_json::from_str(
        r#"{
          "name": "intentional-default-branch",
          "roles": [{"id": "architect"}],
          "labels": [{"id": "feature"}, {"id": "landing"}],
          "artifact_kinds": [
            {"id": "feature", "target": "issue", "identifying_labels": ["feature"]},
            {"id": "landing_pr", "target": "pull_request", "identifying_labels": ["landing"]}
          ],
          "transitions": [
            {"id": "create_default_child", "artifact": "feature", "roles": ["architect"], "effects": [
              {"kind": "create_issues", "target_branch_policy": "repository_default"}
            ]},
            {"id": "converge_without_pr", "artifact": "feature", "roles": ["architect"], "effects": [
              {"kind": "create_pull_request", "artifact_kind": "landing_pr", "target_branch_policy": "repository_default"}
            ]}
          ]
        }"#,
    )
    .expect("repository-default fixture parses");
    let compiled = spec
        .validate()
        .expect("repository-default fixture validates")
        .compile();

    assert!(matches!(
        effect(&compiled, "create_default_child", |effect| matches!(
            effect,
            Effect::CreateIssues { .. }
        )),
        Effect::CreateIssues {
            target_branch_policy: Some(TargetBranchPolicy::RepositoryDefault),
            ..
        }
    ));
    let convergence = compiled
        .role(&RoleId::new("architect"))
        .expect("architect role compiles")
        .tools
        .iter()
        .find(|tool| tool.name == "converge_without_pr")
        .expect("same-branch convergence tool compiles");
    assert!(matches!(
        &convergence.effects[0],
        Effect::CreatePullRequest {
            target_branch_policy: Some(TargetBranchPolicy::RepositoryDefault),
            ..
        }
    ));
}
