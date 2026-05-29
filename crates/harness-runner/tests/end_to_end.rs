//! L2 end-to-end scenarios over the in-process memory stage.

mod support;

use harness_runner::run_scenario_with_budget;

use support::agents::{
    fake_registry, fake_registry_with, ClosingArchitect, FakeArchitect, FakeReviewer,
    RequestChangesThenApproveReviewer,
};
use support::block_on;
use support::ci::{FailThenPassCiPolicy, FixedCiPolicy};
use support::runner_config;
use support::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, dependency_chain_mechanically_unblocked,
    happy_path,
};
use support::world::{full_reference_stage, full_reference_stage_with};

const HAPPY_PATH_BUDGET: u64 = 32;
const VARIANT_BUDGET: u64 = 96;

#[test]
fn end_to_end_reference_delivery_happy_path_converges() {
    let stage = block_on(full_reference_stage(runner_config())).expect("stage builds");
    let scenario = happy_path();

    // Expected fake-driven loop: intake -> triage_to_code (architect) ->
    // claim_code + create PR + request_review (engineer) -> CI passes
    // (CiWorker) + approve_review (reviewer) -> approve_merge (owner) ->
    // reconcile_landed (architect) -> quiescent. The scenario deliberately
    // accepts two current seams: the produced code issue may stay open after
    // merge, and the single PR keeps `alignment` because owner_alignment needs
    // a cohort of five (or max_age) before it activates.
    assert_scenario_converges(&stage, &scenario, HAPPY_PATH_BUDGET);
}

#[test]
fn changes_requested_then_approved_converges_without_premature_merge() {
    let stage = block_on(full_reference_stage_with(
        runner_config(),
        fake_registry_with(FakeArchitect, RequestChangesThenApproveReviewer::new()),
        FixedCiPolicy::pass(),
    ))
    .expect("stage builds");
    let scenario = changes_requested_then_approved();

    assert_scenario_converges(&stage, &scenario, VARIANT_BUDGET);
}

#[test]
fn ci_fails_then_passes_converges_without_premature_merge() {
    let stage = block_on(full_reference_stage_with(
        runner_config(),
        fake_registry(),
        FailThenPassCiPolicy,
    ))
    .expect("stage builds");
    let scenario = ci_fails_then_passes();

    assert_scenario_converges(&stage, &scenario, VARIANT_BUDGET);
}

#[test]
fn dependency_chain_is_mechanically_unblocked_and_merged() {
    let stage = block_on(full_reference_stage_with(
        runner_config(),
        fake_registry_with(ClosingArchitect, FakeReviewer),
        FixedCiPolicy::pass(),
    ))
    .expect("stage builds");
    let scenario = dependency_chain_mechanically_unblocked();

    assert_scenario_converges(&stage, &scenario, VARIANT_BUDGET);
}

fn assert_scenario_converges<S: harness_runner::Stage>(
    stage: &S,
    scenario: &harness_runner::Scenario,
    budget: u64,
) {
    let report =
        block_on(run_scenario_with_budget(stage, scenario, budget)).expect("scenario passes");

    assert!(
        report.ticks <= budget,
        "scenario used {} ticks, over budget {budget}",
        report.ticks
    );
}
