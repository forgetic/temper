// SPDX-License-Identifier: MPL-2.0

//! Freshness predicate for in-flight PR-head jobs.
//!
//! A `pull_request_writable` job is assigned against a specific PR head and a
//! queue condition. Before that job publishes more work (checkpoint/final push)
//! or the daemon applies late progress/results, the current Forge state must
//! still describe the same actionable PR.

use temper_forge::{
    CiJobQuery, Forge, ItemNumber, PullRequestReviewStatus, PullRequestState, RepositoryId,
};
use temper_protocol_worker::{
    PullRequestFreshness, PullRequestFreshnessResponse, PullRequestFreshnessStatus,
};
use temper_workflow::{CiStatus, ReviewStatus};

/// Revalidates assignment-time PR facts against fresh Forge state.
pub async fn check_pull_request_freshness<F: Forge + ?Sized>(
    forge: &F,
    check: &PullRequestFreshness,
) -> PullRequestFreshnessResponse {
    check_pull_request_freshness_inner(forge, check, None).await
}

/// Revalidates a PR-head job that may already have pushed one of its own heads.
///
/// The assignment-time check remains strict until the first successful push by
/// this in-flight job. Once the caller can name the last self-pushed SHA, the
/// PR is fresh when its current head still equals that SHA and the PR identity
/// and open state still match; queue conditions such as `ci_failed` are then no
/// longer required because CI may legitimately be pending on the new head.
pub async fn check_pull_request_freshness_with_self_pushed_head<F: Forge + ?Sized>(
    forge: &F,
    check: &PullRequestFreshness,
    self_pushed_head_sha: Option<&str>,
) -> PullRequestFreshnessResponse {
    check_pull_request_freshness_inner(forge, check, self_pushed_head_sha).await
}

async fn check_pull_request_freshness_inner<F: Forge + ?Sized>(
    forge: &F,
    check: &PullRequestFreshness,
    self_pushed_head_sha: Option<&str>,
) -> PullRequestFreshnessResponse {
    let repo = RepositoryId::new(check.repository_id.clone());
    let number = ItemNumber::new(check.number);
    let pull_request = match forge.get_pull_request_by_number(&repo, number).await {
        Ok(Some(pull_request)) => pull_request,
        Ok(None) => {
            return PullRequestFreshnessResponse::stale(format!(
                "pull request #{} no longer exists",
                check.number
            ));
        }
        Err(error) => {
            return PullRequestFreshnessResponse::unavailable(format!(
                "read pull request #{}: {error}",
                check.number
            ));
        }
    };

    if pull_request.id.as_str() != check.pull_request_id {
        return PullRequestFreshnessResponse::stale(format!(
            "pull request #{} identity changed",
            check.number
        ));
    }

    if pull_request.state != PullRequestState::Open {
        return PullRequestFreshnessResponse::stale(format!(
            "pull request #{} is {:?}",
            check.number, pull_request.state
        ));
    }

    if self_pushed_head_matches(check, self_pushed_head_sha, &pull_request) {
        return PullRequestFreshnessResponse::fresh();
    }

    if pull_request.head_sha != check.head_sha {
        return PullRequestFreshnessResponse::stale(format!(
            "pull request #{} head changed from {} to {}",
            check.number,
            display_sha(check.head_sha.as_deref()),
            display_sha(pull_request.head_sha.as_deref())
        ));
    }

    for label in &check.queue_labels {
        if !pull_request.labels.iter().any(|current| current == label) {
            return PullRequestFreshnessResponse::stale(format!(
                "pull request #{} no longer has queue label `{label}`",
                check.number
            ));
        }
    }

    match check.queue_condition.as_deref() {
        Some("ci_failed") => ci_failed_still_holds(forge, &repo, check, &pull_request).await,
        Some("review_changes_requested") => {
            review_changes_requested_still_holds(forge, check, &pull_request).await
        }
        Some("ci_passed") => ci_passed_still_holds(forge, &repo, check, &pull_request).await,
        Some("review_approved") => review_approved_still_holds(forge, check, &pull_request).await,
        Some(other) => PullRequestFreshnessResponse::unavailable(format!(
            "unsupported PR freshness queue condition `{other}`"
        )),
        None => PullRequestFreshnessResponse::fresh(),
    }
}

fn self_pushed_head_matches(
    check: &PullRequestFreshness,
    self_pushed_head_sha: Option<&str>,
    pull_request: &temper_forge::PullRequest,
) -> bool {
    let Some(self_pushed_head_sha) = self_pushed_head_sha.and_then(non_empty) else {
        return false;
    };
    let Some(current_head) = pull_request.head_sha.as_deref().and_then(non_empty) else {
        return false;
    };
    if current_head != self_pushed_head_sha {
        return false;
    }
    // A caller naming the assignment head has not proven a self-push; keep the
    // original queue-condition checks in force for that case.
    check.head_sha.as_deref().and_then(non_empty) != Some(self_pushed_head_sha)
}

fn display_sha(sha: Option<&str>) -> &str {
    sha.and_then(non_empty).unwrap_or("<none>")
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

async fn ci_failed_still_holds<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    check: &PullRequestFreshness,
    pull_request: &temper_forge::PullRequest,
) -> PullRequestFreshnessResponse {
    match current_ci_status(forge, repo, pull_request).await {
        Ok(status) if status.is_failed() => PullRequestFreshnessResponse::fresh(),
        Ok(status) => PullRequestFreshnessResponse::stale(format!(
            "pull request #{} current-head CI is {:?}, not failed",
            check.number,
            status.state()
        )),
        Err(error) => PullRequestFreshnessResponse::unavailable(format!(
            "read CI for pull request #{}: {error}",
            check.number
        )),
    }
}

async fn ci_passed_still_holds<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    check: &PullRequestFreshness,
    pull_request: &temper_forge::PullRequest,
) -> PullRequestFreshnessResponse {
    match current_ci_status(forge, repo, pull_request).await {
        Ok(status) if status.is_passed() => PullRequestFreshnessResponse::fresh(),
        Ok(status) => PullRequestFreshnessResponse::stale(format!(
            "pull request #{} current-head CI is {:?}, not passed",
            check.number,
            status.state()
        )),
        Err(error) => PullRequestFreshnessResponse::unavailable(format!(
            "read CI for pull request #{}: {error}",
            check.number
        )),
    }
}

async fn current_ci_status<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    pull_request: &temper_forge::PullRequest,
) -> Result<CiStatus, temper_forge::ForgeError> {
    let jobs = forge
        .list_ci_jobs(
            repo,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                commit_sha: pull_request.head_sha.clone(),
                ..CiJobQuery::default()
            },
        )
        .await?;
    Ok(CiStatus::from_jobs(&jobs))
}

async fn review_changes_requested_still_holds<F: Forge + ?Sized>(
    forge: &F,
    check: &PullRequestFreshness,
    pull_request: &temper_forge::PullRequest,
) -> PullRequestFreshnessResponse {
    match current_review_status(forge, pull_request).await {
        Ok(status) if status.has_changes_requested() => PullRequestFreshnessResponse::fresh(),
        Ok(_) => PullRequestFreshnessResponse::stale(format!(
            "pull request #{} no longer has changes requested",
            check.number
        )),
        Err(error) => PullRequestFreshnessResponse::unavailable(format!(
            "read reviews for pull request #{}: {error}",
            check.number
        )),
    }
}

async fn review_approved_still_holds<F: Forge + ?Sized>(
    forge: &F,
    check: &PullRequestFreshness,
    pull_request: &temper_forge::PullRequest,
) -> PullRequestFreshnessResponse {
    match current_review_status(forge, pull_request).await {
        Ok(status) if status.is_approved() => PullRequestFreshnessResponse::fresh(),
        Ok(_) => PullRequestFreshnessResponse::stale(format!(
            "pull request #{} is no longer review-approved",
            check.number
        )),
        Err(error) => PullRequestFreshnessResponse::unavailable(format!(
            "read reviews for pull request #{}: {error}",
            check.number
        )),
    }
}

async fn current_review_status<F: Forge + ?Sized>(
    forge: &F,
    pull_request: &temper_forge::PullRequest,
) -> Result<ReviewStatus, temper_forge::ForgeError> {
    let reviews = forge.list_pull_request_reviews(&pull_request.id).await?;
    let aggregate =
        PullRequestReviewStatus::from_reviews(&pull_request.requested_reviewers, &reviews);
    Ok(ReviewStatus::from_aggregate(&aggregate))
}

/// Convenience for daemon-side stale drops.
pub fn is_stale(response: &PullRequestFreshnessResponse) -> bool {
    response.status == PullRequestFreshnessStatus::Stale
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use temper_forge::{
        BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreatePullRequest,
        CreateRepository, Forge, PullRequestUpdateState, UpdatePullRequest,
    };
    use temper_forge_memory::MemoryForge;

    async fn setup() -> (MemoryForge, RepositoryId, temper_forge::PullRequest) {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository created")
            .id;
        let pr = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement".to_string(),
                    body: "body".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-1".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request created");
        let pr = forge
            .set_pull_request_head(&pr.id, Some("assigned-head".to_string()))
            .expect("pull request head seeded");
        (forge, repo, pr)
    }

    fn check(repo: &RepositoryId, pr: &temper_forge::PullRequest) -> PullRequestFreshness {
        PullRequestFreshness {
            repository_id: repo.as_str().to_string(),
            repo: "ai/temper".to_string(),
            role: "engineer".to_string(),
            queue: "pr_ci_failed".to_string(),
            action: "address_ci_failure".to_string(),
            number: pr.number.get(),
            pull_request_id: pr.id.as_str().to_string(),
            head_sha: pr.head_sha.clone(),
            queue_condition: Some("ci_failed".to_string()),
            queue_labels: Vec::new(),
        }
    }

    fn ci_job(
        repo: &RepositoryId,
        pr: &temper_forge::PullRequest,
        conclusion: CiJobConclusion,
    ) -> CiJob {
        ci_job_for_sha(
            repo,
            pr,
            pr.head_sha.clone().unwrap_or_default(),
            CiJobStatus::Completed,
            Some(conclusion),
        )
    }

    fn ci_job_for_sha(
        repo: &RepositoryId,
        pr: &temper_forge::PullRequest,
        commit_sha: impl Into<String>,
        status: CiJobStatus,
        conclusion: Option<CiJobConclusion>,
    ) -> CiJob {
        let now = chrono::Utc.timestamp_opt(1, 0).unwrap();
        CiJob {
            id: CiJobId::new(format!("ci-{status:?}-{conclusion:?}")),
            repo_id: repo.clone(),
            pull_request_id: Some(pr.id.clone()),
            commit_sha: commit_sha.into(),
            name: "validate".to_string(),
            status,
            conclusion,
            url: None,
            created_at: now,
            started_at: None,
            completed_at: Some(now),
            updated_at: now,
        }
    }

    #[test]
    fn ci_failed_pr_is_fresh() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            forge.seed_ci_jobs(&repo, vec![ci_job(&repo, &pr, CiJobConclusion::Failure)]);

            let response = check_pull_request_freshness(&forge, &check(&repo, &pr)).await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Fresh);
        });
    }

    #[test]
    fn passed_ci_makes_pr_stale_for_ci_failed_job() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            forge.seed_ci_jobs(&repo, vec![ci_job(&repo, &pr, CiJobConclusion::Success)]);

            let response = check_pull_request_freshness(&forge, &check(&repo, &pr)).await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Stale);
            assert!(response.reason.unwrap().contains("not failed"));
        });
    }

    #[test]
    fn closed_pr_is_stale() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            forge
                .update_pull_request(
                    &pr.id,
                    UpdatePullRequest {
                        state: Some(PullRequestUpdateState::Closed),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .expect("close pull request");

            let response = check_pull_request_freshness(&forge, &check(&repo, &pr)).await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Stale);
            assert!(response.reason.unwrap().contains("Closed"));
        });
    }

    #[test]
    fn head_mismatch_is_stale() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            let mut check = check(&repo, &pr);
            check.head_sha = Some("old-head".to_string());

            let response = check_pull_request_freshness(&forge, &check).await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Stale);
            assert!(response.reason.unwrap().contains("head changed"));
        });
    }

    #[test]
    fn self_pushed_head_is_fresh_when_current_ci_is_pending() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            let check = check(&repo, &pr);
            let self_head = "checkpoint-head";
            forge
                .set_pull_request_head(&pr.id, Some(self_head.to_string()))
                .expect("advance PR head");
            forge.seed_ci_jobs(
                &repo,
                vec![ci_job_for_sha(
                    &repo,
                    &pr,
                    self_head,
                    CiJobStatus::Queued,
                    None,
                )],
            );

            let response =
                check_pull_request_freshness_with_self_pushed_head(&forge, &check, Some(self_head))
                    .await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Fresh);
        });
    }

    #[test]
    fn external_head_after_self_push_is_stale() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            let check = check(&repo, &pr);
            forge
                .set_pull_request_head(&pr.id, Some("external-head".to_string()))
                .expect("advance PR head externally");

            let response = check_pull_request_freshness_with_self_pushed_head(
                &forge,
                &check,
                Some("checkpoint-head"),
            )
            .await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Stale);
            assert!(response.reason.unwrap().contains("head changed"));
        });
    }

    #[test]
    fn assignment_head_candidate_still_requires_failed_ci() {
        temper_engine_io::block_on(async {
            let (forge, repo, pr) = setup().await;
            let check = check(&repo, &pr);
            forge.seed_ci_jobs(&repo, vec![ci_job(&repo, &pr, CiJobConclusion::Success)]);
            let assignment_head = pr.head_sha.as_deref().expect("assigned head");

            let response = check_pull_request_freshness_with_self_pushed_head(
                &forge,
                &check,
                Some(assignment_head),
            )
            .await;

            assert_eq!(response.status, PullRequestFreshnessStatus::Stale);
            assert!(response.reason.unwrap().contains("not failed"));
        });
    }
}
