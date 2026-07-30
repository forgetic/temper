// SPDX-License-Identifier: MPL-2.0

const CI_WORKFLOW: &str = include_str!("../.forgejo/workflows/ci.yml");
const FOCUSED_WORKFLOW: &str = include_str!("../.forgejo/workflows/focused-feature-validation.yml");
const POST_MERGE_WORKFLOW: &str = include_str!("../.forgejo/workflows/post-merge-validation.yml");

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
            .any(|line| line.trim() == "CARGO_BUILD_JOBS: \"2\""),
        "the shared runner must preserve memory and linker disk headroom"
    );
}

#[test]
fn repository_ci_reclaims_oversized_shared_cache_before_disk_consumers() {
    let validate_job = CI_WORKFLOW
        .split_once("  validate:\n")
        .expect("CI declares the validate job")
        .1
        .split_once("  web:\n")
        .expect("validate precedes web")
        .0;
    let web_job = CI_WORKFLOW
        .split_once("  web:\n")
        .expect("CI declares the web job")
        .1;

    for (job, first_consumer) in [
        (validate_job, "      - name: Prepare scratch directory\n"),
        (web_job, "      - name: Install (npm ci)\n"),
    ] {
        let gc_offset = job
            .find("      - name: Reclaim shared build cache\n")
            .expect("host job explicitly runs build-cache LRU eviction");
        let consumer_offset = job
            .find(first_consumer)
            .expect("host job declares its first disk consumer");
        assert!(
            gc_offset < consumer_offset,
            "shared cache must be size-bounded before the job consumes disk"
        );
        let gc_step = &job[gc_offset..consumer_offset];
        assert!(
            gc_step.lines().any(|line| line.trim() == "run: kache gc"),
            "shared cache reclamation must use configured LRU eviction"
        );
    }
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
fn repository_ci_runs_one_mapped_scenario_from_exact_feature_head() {
    let trigger = FOCUSED_WORKFLOW
        .split_once("  pull_request:\n")
        .expect("focused validation declares a pull_request trigger")
        .1
        .split_once("jobs:\n")
        .expect("focused validation trigger precedes jobs")
        .0;
    assert!(
        trigger.lines().any(|line| line.trim() == "- main"),
        "focused validation must select aggregate PRs targeting main"
    );
    let focused_job = FOCUSED_WORKFLOW
        .split_once("  focused-feature-validation:\n")
        .expect("CI declares focused feature validation")
        .1;

    assert!(
        focused_job.contains("ref: ${{ github.event.pull_request.head.sha }}")
            && focused_job.contains("fetch-depth: 0"),
        "focused validation must check out the exact head with its landing base available"
    );
    let run_step = focused_job
        .split_once("      - name: Resolve and run one mapped feature scenario\n")
        .expect("focused job resolves and runs a mapping")
        .1
        .split_once("      - name: Upload focused exact-head evidence\n")
        .expect("focused run precedes artifact upload")
        .0;
    assert!(
        run_step.contains("s#^agent/pr-for-feature-\\([1-9][0-9]*\\)$#\\1#p")
            && run_step.contains("s#^feature/\\([1-9][0-9]*\\)\\(-.*\\)\\?$#\\1#p"),
        "focused CI must derive feature identity from canonical and legacy feature branches"
    );
    assert!(
        run_step.contains("cargo dev-scenario-validate-feature")
            && run_step.contains("--landing-base \"$FEATURE_LANDING_BASE_SHA\"")
            && run_step.contains("--sha \"$FEATURE_HEAD_SHA\"")
            && !run_step.contains("--scenario"),
        "CI must resolve the sole mapped scenario instead of naming or defaulting one"
    );
}

#[test]
fn live_scenario_lanes_reuse_the_host_fixture_cache() {
    let cache_export = "export BENCH_FORGEJO_CACHE_DIR=\"$HOME/.cache/bench-forgejo\"";
    assert!(
        FOCUSED_WORKFLOW.contains(cache_export),
        "focused validation must not download Forgejo fixtures during the gate"
    );
    assert!(
        POST_MERGE_WORKFLOW.contains(cache_export),
        "post-merge validation must not download Forgejo fixtures during the report"
    );
}

#[test]
fn repository_ci_preserves_focused_evidence_on_failure_and_broad_coverage() {
    let focused_job = FOCUSED_WORKFLOW
        .split_once("  focused-feature-validation:\n")
        .expect("CI declares focused feature validation")
        .1;
    let upload = focused_job
        .split_once("      - name: Upload focused exact-head evidence\n")
        .expect("focused job uploads evidence")
        .1
        .split_once("      - name: Cleanup focused scratch directory\n")
        .expect("upload precedes cleanup")
        .0;
    assert!(upload.contains("if: ${{ always() }}"));
    assert!(upload.contains("path: ${{ env.FOCUSED_ARTIFACT_DIR }}/"));

    assert!(CI_WORKFLOW.contains("run: cargo dev-scenario-check"));
    assert!(CI_WORKFLOW.contains("scripts/run-nextest-quick.sh --run-ignored only -P e2e"));
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
