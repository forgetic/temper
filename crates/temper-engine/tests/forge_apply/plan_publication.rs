// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_protocol_worker::{JobPlanPublication, JobPlanPublicationTarget, JobProgress};

#[test]
fn early_plan_publication_creates_in_progress_pr_once() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let correlation = format!("pr-for-code-{}", issue.get());
        let branch = format!("agent/{correlation}");
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &correlation,
            vec![writable_repo("acme/service", &branch)],
        );
        let progress = plan_progress(
            &correlation,
            "Plan the daemon change",
            ["Write failing test", "Implement fix"],
            [plan_target("acme/service", "main", &branch)],
        );

        applier.apply_progress(job.clone(), progress.clone()).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull = &pulls[0];
        assert_eq!(pull.source.branch, branch);
        assert_eq!(pull.target.branch, "main");
        assert_eq!(pull.labels, vec!["implementation", "in-progress"]);
        assert!(!pull.labels.iter().any(|label| label == "needs-reviewer"));
        assert!(pull.requested_reviewers.is_empty());
        assert!(pull.body.contains("Summary: Plan the daemon change"));
        assert!(
            pull.body
                .contains("Implementation plan:\n\n- [ ] Write failing test\n- [ ] Implement fix")
        );
        let metadata = parse_metadata_block(&pull.body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");
        assert_eq!(metadata.parents, vec![ArtifactRef::same_repo(issue)]);
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(correlation.as_str())
        );

        let body = pull.body.clone();
        let number = pull.number;
        applier.apply_progress(job, progress).await;

        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
        let replayed = pull_request(&forge, &repo, number).await;
        assert_eq!(replayed.body, body);
        assert_issue_comments_stay_empty(&cx, &forge, &repo, issue).await;
    })
}

#[test]
fn phase_progress_and_final_success_reuse_plan_pr_preserving_checks() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let correlation = format!("pr-for-code-{}", issue.get());
        let branch = format!("agent/{correlation}");
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &correlation,
            vec![writable_repo("acme/service", &branch)],
        );

        applier
            .apply_progress(
                job.clone(),
                plan_progress(
                    &correlation,
                    "Plan first summary",
                    ["Write failing test", "Implement fix"],
                    [plan_target("acme/service", "main", &branch)],
                ),
            )
            .await;
        let pull_number = wait_for_pull_request_count(&cx, &forge, &repo, 1).await[0].number;

        applier
            .apply_progress(
                job.clone(),
                phase_progress(&correlation, 2, "Implement fix", Some("abc123")),
            )
            .await;
        let checked_body = pull_request(&forge, &repo, pull_number).await.body;
        assert!(checked_body.contains("- [ ] Write failing test\n- [x] Implement fix"));

        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch,
            "implemented final product",
        );
        result.details = Some(json!({
            "plan": {"phases": ["Write failing test", "Implement fix"]}
        }));
        applier.apply(job.clone(), result.clone()).await;

        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
        let finalized = pull_request(&forge, &repo, pull_number).await;
        assert_eq!(finalized.labels, vec!["implementation", "needs-reviewer"]);
        assert_eq!(finalized.requested_reviewers, vec![UserId::new("reviewer")]);
        assert!(
            finalized
                .body
                .contains("Summary: implemented final product")
        );
        assert!(
            finalized
                .body
                .contains("- [ ] Write failing test\n- [x] Implement fix")
        );
        parse_metadata_block(&finalized.body)
            .expect("PR metadata parses")
            .expect("PR metadata exists");

        applier.apply(job, result).await;
        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;
        let replayed = pull_request(&forge, &repo, pull_number).await;
        assert_eq!(replayed.body, finalized.body);
        assert_eq!(replayed.labels, finalized.labels);
        assert_eq!(replayed.requested_reviewers, finalized.requested_reviewers);
        assert_issue_comments_stay_empty(&cx, &forge, &repo, issue).await;
    })
}

#[test]
fn one_phase_plan_publication_stays_plain_and_quiet() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let correlation = format!("pr-for-code-{}", issue.get());
        let branch = format!("agent/{correlation}");
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &correlation,
            vec![writable_repo("acme/service", &branch)],
        );

        applier
            .apply_progress(
                job.clone(),
                plan_progress(
                    &correlation,
                    "One obvious edit",
                    ["Apply obvious edit"],
                    [plan_target("acme/service", "main", &branch)],
                ),
            )
            .await;
        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull_number = pulls[0].number;
        let body = pulls[0].body.clone();
        assert!(!body.contains("Implementation plan"));
        assert!(!body.contains("- [ ]"));

        applier
            .apply_progress(
                job,
                phase_progress(&correlation, 1, "Apply obvious edit", Some("abc123")),
            )
            .await;

        assert_eq!(pull_request(&forge, &repo, pull_number).await.body, body);
        assert_issue_comments_stay_empty(&cx, &forge, &repo, issue).await;
    })
}

#[test]
fn plan_publication_opens_selected_repos_with_dependency_metadata() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let primary = create_repo(&forge, "acme", "service", "main").await;
        let secondary = create_repo(&forge, "acme", "lib", "main").await;
        let issue = create_ready_issue(&forge, &primary).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let correlation = format!("coord-for-code-{}", issue.get());
        let branch = format!("agent/{correlation}");
        let mut lib = writable_repo("acme/lib", &branch);
        lib.depends_on = vec!["acme/service".to_string()];
        let job = coordinated_in_flight_job(
            "acme/service",
            issue,
            &correlation,
            vec![writable_repo("acme/service", &branch), lib],
        );

        applier
            .apply_progress(
                job,
                plan_progress(
                    &correlation,
                    "Cross-repo plan",
                    ["Update service", "Update lib"],
                    [
                        plan_target("acme/service", "main", &branch),
                        plan_target("acme/lib", "main", &branch),
                    ],
                ),
            )
            .await;

        let primary_pulls = wait_for_pull_request_count(&cx, &forge, &primary, 1).await;
        let secondary_pulls = wait_for_pull_request_count(&cx, &forge, &secondary, 1).await;
        let primary_pull = &primary_pulls[0];
        let secondary_pull = &secondary_pulls[0];
        assert_eq!(primary_pull.labels, vec!["implementation", "in-progress"]);
        assert_eq!(secondary_pull.labels, vec!["implementation", "in-progress"]);
        let secondary_meta = parse_metadata_block(&secondary_pull.body)
            .expect("secondary metadata parses")
            .expect("secondary metadata exists");
        assert_eq!(
            secondary_meta.parents,
            vec![ArtifactRef::in_repo(primary.clone(), issue)]
        );
        assert_eq!(
            secondary_meta.dependencies,
            vec![ArtifactRef::in_repo(primary, primary_pull.number)]
        );
        assert_eq!(
            secondary_meta.correlation_key.as_deref(),
            Some(correlation.as_str())
        );
    })
}

fn plan_progress<const P: usize, const T: usize>(
    correlation_key: &str,
    summary: &str,
    phases: [&str; P],
    targets: [JobPlanPublicationTarget; T],
) -> JobProgress {
    JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        correlation_key: correlation_key.to_string(),
        step: 1,
        status: "publish implementation plan".to_string(),
        state: "done".to_string(),
        pushed_sha: Some("abc123".to_string()),
        note: Some(summary.to_string()),
        plan_publication: Some(JobPlanPublication {
            summary: summary.to_string(),
            phases: phases.iter().map(|phase| (*phase).to_string()).collect(),
            target_repos: targets.into_iter().collect(),
        }),
    }
}

fn phase_progress(
    correlation_key: &str,
    step: u32,
    status: &str,
    pushed_sha: Option<&str>,
) -> JobProgress {
    JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        correlation_key: correlation_key.to_string(),
        step,
        status: status.to_string(),
        state: "done".to_string(),
        pushed_sha: pushed_sha.map(str::to_string),
        note: Some("checkpoint done".to_string()),
        plan_publication: None,
    }
}

fn plan_target(repo: &str, base_branch: &str, branch: &str) -> JobPlanPublicationTarget {
    JobPlanPublicationTarget {
        repo_path: repo.to_string(),
        dir: repo.rsplit('/').next().unwrap_or(repo).to_string(),
        base_branch: base_branch.to_string(),
        branch_hint: Some(branch.to_string()),
    }
}

async fn pull_request(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> PullRequest {
    forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request lookup succeeds")
        .expect("pull request exists")
}
