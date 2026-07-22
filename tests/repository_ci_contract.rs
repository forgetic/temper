// SPDX-License-Identifier: MPL-2.0

const CI_WORKFLOW: &str = include_str!("../.forgejo/workflows/ci.yml");

#[test]
fn repository_ci_accepts_feature_branch_pull_request_targets() {
    let workflow = CI_WORKFLOW;
    let trigger = workflow
        .split_once("  pull_request:\n")
        .expect("CI declares a pull_request trigger")
        .1
        .split_once("jobs:\n")
        .expect("CI trigger precedes jobs")
        .0;

    assert!(
        !trigger.lines().any(|line| line.trim() == "branches:"),
        "a target-branch filter would strand plan-centric implementation PRs without CI"
    );
    for event in ["opened", "synchronize", "reopened"] {
        assert!(
            trigger
                .lines()
                .any(|line| line.trim() == format!("- {event}")),
            "pull_request trigger must retain `{event}` events"
        );
    }
}

#[test]
fn repository_ci_bounds_parallel_rust_builds_on_shared_host() {
    let validate_job = CI_WORKFLOW
        .split_once("  validate:\n")
        .expect("CI declares the validate job")
        .1
        .split_once("    steps:\n")
        .expect("validate job declares steps")
        .0;

    assert!(
        validate_job
            .lines()
            .any(|line| line.trim() == "CARGO_BUILD_JOBS: \"4\""),
        "the no-swap shared runner must bound parallel rustc/linker processes"
    );
}

#[test]
fn repository_ci_runs_e2e_from_repaired_captured_binaries() {
    let e2e_step = CI_WORKFLOW
        .split_once("      - name: Test (all e2e)\n")
        .expect("CI declares the all-e2e step")
        .1
        .split_once("      - name: Free linked test binaries\n")
        .expect("the all-e2e step precedes linked-test cleanup")
        .0;

    assert!(
        e2e_step.lines().any(|line| {
            line.trim() == "scripts/run-nextest-quick.sh --run-ignored only -P e2e"
        }),
        "the e2e lane must repair cached custom-harness modes after its Cargo build"
    );
    assert!(
        !e2e_step
            .lines()
            .any(|line| line.trim() == "cargo dev-test-e2e-all"),
        "a direct nextest alias can restore non-executable harnesses after permission repair"
    );
}

#[test]
fn repository_ci_installs_locked_web_dependencies_without_advisory_network_calls() {
    let install_step = CI_WORKFLOW
        .split_once("      - name: Install (npm ci)\n")
        .expect("CI declares the web dependency install step")
        .1
        .split_once("      - name: Test (vitest)\n")
        .expect("the web dependency install precedes tests")
        .0;

    assert!(
        install_step
            .lines()
            .any(|line| line.trim() == "run: npm ci --no-audit --no-fund"),
        "web dependency installation must avoid non-gating advisory and funding network calls"
    );
}

#[test]
fn repository_ci_reclaims_linked_test_binaries_after_failure() {
    let cleanup_step = CI_WORKFLOW
        .split_once("      - name: Free linked test binaries\n")
        .expect("CI declares linked-test cleanup")
        .1
        .split_once("      - name: Lint\n")
        .expect("linked-test cleanup precedes lint")
        .0;

    assert!(
        cleanup_step
            .lines()
            .any(|line| line.trim() == "if: ${{ always() }}"),
        "linked test binaries must be reclaimed even when an earlier validation step fails"
    );
}
