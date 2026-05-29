//! L2 end-to-end scenarios over the in-process memory stage.

mod support;

use harness_runner::run_scenario_with_budget;

use support::block_on;
use support::runner_config;
use support::scenarios::happy_path;
use support::world::full_reference_stage;

const HAPPY_PATH_BUDGET: u64 = 32;

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
    let report = block_on(run_scenario_with_budget(
        &stage,
        &scenario,
        HAPPY_PATH_BUDGET,
    ))
    .expect("happy path scenario passes");

    assert!(
        report.ticks <= HAPPY_PATH_BUDGET,
        "scenario used {} ticks, over budget {HAPPY_PATH_BUDGET}",
        report.ticks
    );
}
