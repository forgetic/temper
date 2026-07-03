// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn success_result_creates_implementation_pr_targeting_manifest_base_branch() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let branch_name = format!("agent/pr-for-code-{}", issue.get());
        let mut manifest_repo = writable_repo("acme/service", &branch_name);
        manifest_repo.default_branch = "stable".to_string();
        manifest_repo.base_branch = "feature/144-plan-branch".to_string();
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &format!("pr-for-code-{}", issue.get()),
            vec![manifest_repo],
        );

        applier
            .apply(
                job.clone(),
                success_result(
                    "worker-a",
                    &job.job_id,
                    &job.repo,
                    &branch_name,
                    "implemented feature branch targeting",
                ),
            )
            .await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        assert_eq!(pulls[0].target.branch, "feature/144-plan-branch");
        assert_eq!(pulls[0].source.branch, branch_name);
    })
}
