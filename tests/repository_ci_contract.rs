// SPDX-License-Identifier: MPL-2.0

#[test]
fn repository_ci_accepts_feature_branch_pull_request_targets() {
    let workflow = include_str!("../.forgejo/workflows/ci.yml");
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
