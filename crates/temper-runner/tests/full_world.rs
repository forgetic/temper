//! Composition smoke tests for the full in-process reference-delivery world.

use temper_runner::Stage;

use temper_testing::block_on;
use temper_testing::runner_config;
use temper_testing::world::full_reference_stage;

#[test]
fn fully_wired_memory_stage_quiesces_on_empty_repo() {
    let stage = block_on(full_reference_stage(runner_config())).expect("stage builds");

    let report = block_on(stage.run_to_quiescence(50)).expect("stage converges");

    assert_eq!(report.action_count("mechanical"), Some(0));
    assert_eq!(report.action_count("role:architect"), Some(0));
    assert_eq!(report.action_count("role:engineer"), Some(0));
    assert_eq!(report.action_count("role:reviewer"), Some(0));
    assert_eq!(report.action_count("role:owner"), Some(0));
    assert_eq!(report.action_count("role:human"), Some(0));
    assert_eq!(report.action_count("ci"), Some(0));
}
