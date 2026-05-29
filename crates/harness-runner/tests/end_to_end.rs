//! L2/L3 end-to-end scenarios over in-process memory and filesystem stages.

mod support;

use harness_runner::{run_scenario_with_budget, Scenario, Stage, StageError};

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
use support::world::{
    full_reference_filesystem_stage, full_reference_filesystem_stage_with,
    full_reference_multiprocess_stage, full_reference_stage, full_reference_stage_with,
};

const HAPPY_PATH_BUDGET: u64 = 32;
const VARIANT_BUDGET: u64 = 96;

#[test]
fn end_to_end_reference_delivery_happy_path_converges() {
    let scenario = happy_path();

    // Expected fake-driven loop: intake -> triage_to_code (architect) ->
    // claim_code + create PR + request_review (engineer) -> CI passes
    // (CiWorker) + approve_review (reviewer) -> approve_merge (owner) ->
    // reconcile_landed (architect) -> quiescent. The scenario deliberately
    // accepts two current seams: the produced code issue may stay open after
    // merge, and the single PR keeps `alignment` because owner_alignment needs
    // a cohort of five (or max_age) before it activates.
    assert_scenario_converges_on_backends(
        &scenario,
        HAPPY_PATH_BUDGET,
        || block_on(full_reference_stage(runner_config())),
        || block_on(full_reference_filesystem_stage(runner_config())),
    );

    let multiprocess_stage = block_on(full_reference_multiprocess_stage(runner_config()))
        .expect("multi-process stage builds");
    assert_scenario_converges(
        "filesystem-multiprocess-sketch",
        &multiprocess_stage,
        &scenario,
        HAPPY_PATH_BUDGET,
    );
}

#[test]
fn changes_requested_then_approved_converges_without_premature_merge() {
    let scenario = changes_requested_then_approved();

    assert_scenario_converges_on_backends(
        &scenario,
        VARIANT_BUDGET,
        || {
            block_on(full_reference_stage_with(
                runner_config(),
                fake_registry_with(FakeArchitect, RequestChangesThenApproveReviewer::new()),
                FixedCiPolicy::pass(),
            ))
        },
        || {
            block_on(full_reference_filesystem_stage_with(
                runner_config(),
                fake_registry_with(FakeArchitect, RequestChangesThenApproveReviewer::new()),
                FixedCiPolicy::pass(),
            ))
        },
    );
}

#[test]
fn ci_fails_then_passes_converges_without_premature_merge() {
    let scenario = ci_fails_then_passes();

    assert_scenario_converges_on_backends(
        &scenario,
        VARIANT_BUDGET,
        || {
            block_on(full_reference_stage_with(
                runner_config(),
                fake_registry(),
                FailThenPassCiPolicy,
            ))
        },
        || {
            block_on(full_reference_filesystem_stage_with(
                runner_config(),
                fake_registry(),
                FailThenPassCiPolicy,
            ))
        },
    );
}

#[test]
fn dependency_chain_is_mechanically_unblocked_and_merged() {
    let scenario = dependency_chain_mechanically_unblocked();

    assert_scenario_converges_on_backends(
        &scenario,
        VARIANT_BUDGET,
        || {
            block_on(full_reference_stage_with(
                runner_config(),
                fake_registry_with(ClosingArchitect, FakeReviewer),
                FixedCiPolicy::pass(),
            ))
        },
        || {
            block_on(full_reference_filesystem_stage_with(
                runner_config(),
                fake_registry_with(ClosingArchitect, FakeReviewer),
                FixedCiPolicy::pass(),
            ))
        },
    );
}

fn assert_scenario_converges_on_backends<M, F, MB, FB>(
    scenario: &Scenario,
    budget: u64,
    memory_builder: MB,
    filesystem_builder: FB,
) where
    M: Stage,
    F: Stage,
    MB: FnOnce() -> Result<M, StageError>,
    FB: FnOnce() -> Result<F, StageError>,
{
    let memory_stage = memory_builder().expect("memory stage builds");
    assert_scenario_converges("memory", &memory_stage, scenario, budget);

    let filesystem_stage = filesystem_builder().expect("filesystem stage builds");
    assert_scenario_converges("filesystem", &filesystem_stage, scenario, budget);
}

fn assert_scenario_converges<S: Stage>(backend: &str, stage: &S, scenario: &Scenario, budget: u64) {
    let report = block_on(run_scenario_with_budget(stage, scenario, budget))
        .unwrap_or_else(|error| panic!("{backend} scenario passes: {error}"));

    assert!(
        report.ticks <= budget,
        "{backend} scenario used {} ticks, over budget {budget}",
        report.ticks
    );
}
