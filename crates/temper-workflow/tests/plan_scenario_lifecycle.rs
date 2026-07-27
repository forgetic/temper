// SPDX-License-Identifier: MPL-2.0

use temper_workflow::{
    Diagnostic, Effect, RawEffect, RawWorkflowSpec, RelationKind, TargetBranchPolicy,
};

const PLAN_CENTRIC: &str =
    include_str!("../../../scenarios/plan-centric-feature-branch/config/workflow.json");

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(PLAN_CENTRIC).expect("workflow JSON parses");
    spec.validate().expect("scenario-child workflow validates")
}

#[test]
fn plan_decomposition_compiles_heterogeneous_child_contract() {
    let workflow = workflow();
    let transition = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "plan_children_created")
        .expect("plan child transition");
    let Effect::CreateIssues {
        min_children,
        child_kind_requirements,
        target_branch_policy,
        ..
    } = transition
        .effects
        .iter()
        .find(|effect| matches!(effect, Effect::CreateIssues { .. }))
        .expect("create issues effect")
    else {
        unreachable!()
    };

    assert_eq!(*min_children, 2);
    assert_eq!(*target_branch_policy, Some(TargetBranchPolicy::Inherit));
    assert_eq!(child_kind_requirements.len(), 2);
    let product = &child_kind_requirements[0];
    assert_eq!(product.kind.as_str(), "code");
    assert_eq!(product.min_children, 1);
    let scenario = &child_kind_requirements[1];
    assert_eq!(scenario.kind.as_str(), "validation");
    assert_eq!(scenario.min_children, 1);
    assert_eq!(scenario.max_children, Some(1));
    assert_eq!(scenario.depends_on_all_kinds[0].as_str(), "code");
}

#[test]
fn workflow_validation_rejects_impossible_child_kind_requirement() {
    let mut spec: RawWorkflowSpec =
        serde_json::from_str(PLAN_CENTRIC).expect("workflow JSON parses");
    let transition = spec
        .transitions
        .iter_mut()
        .find(|transition| transition.id == "plan_children_created")
        .expect("plan child transition");
    let RawEffect::CreateIssues {
        child_kind_requirements,
        ..
    } = transition
        .effects
        .iter_mut()
        .find(|effect| matches!(effect, RawEffect::CreateIssues { .. }))
        .expect("create issues effect")
    else {
        unreachable!()
    };
    child_kind_requirements[1].max_children = Some(0);

    let errors = spec
        .validate()
        .expect_err("inverted per-kind cardinality is rejected");
    assert!(errors.diagnostics().iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::InvalidChildKindRequirement { transition, kind, .. }
            if transition == "plan_children_created" && kind == "validation"
    )));
}

#[test]
fn workflow_validation_requires_declared_scenario_product_dependency_relation() {
    let mut spec: RawWorkflowSpec =
        serde_json::from_str(PLAN_CENTRIC).expect("workflow JSON parses");
    spec.relations.retain(|relation| {
        !(relation.kind == RelationKind::Dependency
            && relation.source == "validation"
            && relation.target == "code")
    });

    let errors = spec
        .validate()
        .expect_err("unclassifiable scenario dependency contract is rejected");
    assert!(errors.diagnostics().iter().any(|diagnostic| matches!(
        diagnostic,
        Diagnostic::InvalidChildKindRequirement { transition, kind, reason }
            if transition == "plan_children_created"
                && kind == "validation"
                && reason.contains("dependency relation from `validation` to `code`")
    )));
}

#[test]
fn scenario_author_and_final_validator_have_distinct_checkout_capabilities() {
    let workflow = workflow();
    let scenario_queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "validation_ready")
        .expect("scenario author queue");
    assert_eq!(scenario_queue.actions.len(), 1);
    assert_eq!(scenario_queue.actions[0].role.as_str(), "scenario_author");
    assert_eq!(scenario_queue.actions[0].action.as_str(), "author_scenario");
    assert_eq!(
        scenario_queue.actions[0].checkout.as_deref(),
        Some("writable")
    );

    let validator_queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "plan_needs_validation")
        .expect("final validator queue");
    assert_eq!(validator_queue.actions.len(), 1);
    assert_eq!(validator_queue.actions[0].role.as_str(), "tester");
    assert_eq!(
        validator_queue.actions[0].checkout.as_deref(),
        Some("read_only")
    );

    let scenario_role = workflow
        .roles()
        .iter()
        .find(|role| role.id.as_str() == "scenario_author")
        .expect("scenario author role");
    assert!(
        scenario_role
            .external_tools
            .iter()
            .any(|tool| tool.id.as_str() == "coding_workspace")
    );
    let tester = workflow
        .roles()
        .iter()
        .find(|role| role.id.as_str() == "tester")
        .expect("tester role");
    assert!(
        tester
            .external_tools
            .iter()
            .all(|tool| tool.id.as_str() != "coding_workspace")
    );
}

#[test]
fn plan_readiness_waits_for_scenario_pr_landing_path() {
    let workflow = workflow();
    let unblock = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "blocked_validation_unblock")
        .expect("dependency-gated validation unblock queue");
    assert_eq!(unblock.artifacts[0].as_str(), "validation");
    assert!(unblock.automation.is_some());

    let author = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "author_scenario")
        .expect("scenario author transition");
    assert!(author.effects.iter().any(|effect| matches!(
        effect,
        Effect::CreatePullRequest {
            artifact_kind: Some(kind),
            target_branch_policy: Some(TargetBranchPolicy::NonDefault),
            ..
        } if kind.as_str() == "scenario_pr"
    )));

    let landing = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "land_scenario_pr")
        .expect("scenario landing transition");
    assert!(
        landing
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::CloseParentIssues))
    );

    let plan_ready = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "plan_dependencies_resolved")
        .expect("plan dependency queue");
    assert_eq!(plan_ready.artifacts[0].as_str(), "plan");
    assert_eq!(plan_ready.labels[0].as_str(), "in-progress");
}
