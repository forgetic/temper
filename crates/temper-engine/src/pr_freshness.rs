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

fn display_sha(sha: Option<&str>) -> &str {
    sha.filter(|sha| !sha.is_empty()).unwrap_or("<none>")
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
    let aggregate = PullRequestReviewStatus::from_reviews(&pull_request.requested_reviewers, &reviews);
    Ok(ReviewStatus::from_aggregate(&aggregate))
}

/// Convenience for daemon-side stale drops.
pub fn is_stale(response: &PullRequestFreshnessResponse) -> bool {
    response.status == PullRequestFreshnessStatus::Stale
}
