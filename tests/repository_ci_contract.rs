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
