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

#[test]
fn interrupted_terminal_observation_is_present_recovery_required_with_evidence() {
    let inner = MemoryForge::new();
    let repo = new_repo(&inner);
    let pull_request = create_pr(
        &inner,
        &repo,
        &["implementation", "watch"],
        String::new(),
        Some("head-interrupted"),
    );
    let mut job = ci_job(
        &repo,
        &pull_request,
        "interrupted-1",
        "head-interrupted",
        "validate",
        CiJobStatus::Completed,
        Some(CiJobConclusion::RunnerLost),
        "2026-05-29T00:00:01Z",
        Some("2026-05-29T00:01:01Z"),
    );
    job.provider_conclusion = Some("failure".to_string());
    job.provider_reason = Some("runner disconnected".to_string());
    job.run_id = Some("591".to_string());
    inner.seed_ci_jobs(&repo, vec![job]);
    let workflow = workflow(CI_WORKFLOW);

    let observed = observations(&inner, &repo, &workflow);

    assert_eq!(observed.len(), 1);
    let observation = &observed[0];
    assert!(observation.current_head_jobs_present);
    assert_eq!(observation.state, CiState::RecoveryRequired);
    assert_eq!(observation.terminal_evidence.len(), 1);
    let evidence = &observation.terminal_evidence[0];
    assert_eq!(evidence.conclusion, CiJobConclusion::RunnerLost);
    assert_eq!(
        evidence.provider_reason.as_deref(),
        Some("runner disconnected")
    );
    assert_eq!(evidence.run_id.as_deref(), Some("591"));
}
