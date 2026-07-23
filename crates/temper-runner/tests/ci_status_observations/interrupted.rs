// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn interrupted_missing_ci_parking_is_observed_only_for_its_exact_head() {
    let inner = MemoryForge::new();
    let repo = new_repo(&inner);
    create_pr(
        &inner,
        &repo,
        &["implementation", "watch", "needs-human"],
        render_metadata_block(&WorkflowMetadata {
            missing_ci_recovery: Some(MissingCiRecoveryState {
                head_sha: "recovering-head".to_string(),
                first_observed_at: ts("2026-07-21T11:55:00Z"),
            }),
            ..WorkflowMetadata::default()
        }),
        Some("recovering-head"),
    );
    create_pr(
        &inner,
        &repo,
        &["implementation", "watch", "needs-human"],
        render_metadata_block(&WorkflowMetadata {
            missing_ci_recovery: Some(MissingCiRecoveryState {
                head_sha: "old-head".to_string(),
                first_observed_at: ts("2026-07-21T11:55:00Z"),
            }),
            ..WorkflowMetadata::default()
        }),
        Some("changed-head"),
    );
    let forge = CountingForge::new(inner);
    let workflow = workflow(CI_WORKFLOW);

    let observed = observations(&forge, &repo, &workflow);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].head_sha, "recovering-head");
    assert!(!observed[0].current_head_jobs_present);
    assert_eq!(forge.count(CountedForgeOp::GetPullRequest), 2);
    assert_eq!(forge.count(CountedForgeOp::ListCiJobs), 1);
    assert_eq!(forge.write_count(), 0);
}
