//! Conversions for review verdicts and the merge/review provider tokens.

use super::normalize;
use crate::ids::{RepoCoord, format_review_id, format_user_id};
use crate::types::ReviewDto;
use temper_forge::{
    ForgeError, ForgeResult, MergeMethod, PullRequestId, PullRequestReview, ReviewDecision,
};

/// Maps a Forgejo review DTO into a portable [`PullRequestReview`].
///
/// Returns `None` only for review-request events (`REQUEST_REVIEW`) and any state
/// without a portable decision (`PENDING`, comment-only on some versions).
///
/// **Dismissed and stale reviews are kept.** The reference (filesystem/memory)
/// backends have no dismissal concept and return every verdict event, and the
/// portable review aggregate
/// ([`PullRequestReviewStatus::from_reviews`](temper_forge::PullRequestReviewStatus))
/// already resolves superseding by taking the **latest review per reviewer**.
/// Forgejo, however, auto-dismisses a reviewer's prior review when they submit a
/// new one (e.g. an approval after a changes-requested review), so dropping
/// dismissed reviews here would erase the changes-requested event from history
/// and diverge from the reference contract — breaking history-sensitive
/// consumers while not affecting the gate (the approval is still the latest).
/// Keeping them aligns the backends; see `docs/reference/forge-interface.md`.
pub(crate) fn map_review(
    repo: &RepoCoord,
    pull_request_id: &PullRequestId,
    dto: ReviewDto,
) -> Option<PullRequestReview> {
    let decision = map_review_decision(&dto.state)?;
    let submitted_at = dto.submitted_at.or(dto.updated_at).or(dto.created_at)?;
    Some(PullRequestReview {
        id: format_review_id(repo, dto.id),
        pull_request_id: pull_request_id.clone(),
        reviewer_id: format_user_id(&dto.user.login),
        decision,
        body: dto.body,
        submitted_at,
    })
}

/// Maps a Forgejo review state string to a portable [`ReviewDecision`].
///
/// Accepts both Forgejo's submit event names and its stored state names. Returns
/// `None` for review requests and unknown states.
pub(crate) fn map_review_decision(state: &str) -> Option<ReviewDecision> {
    match normalize(state).as_str() {
        "approved" | "approve" => Some(ReviewDecision::Approved),
        "request_changes" | "changes_requested" => Some(ReviewDecision::ChangesRequested),
        "comment" | "commented" => Some(ReviewDecision::Commented),
        "pending" => Some(ReviewDecision::Pending),
        _ => None,
    }
}

/// Returns the Forgejo submit event token for a portable review decision.
///
/// Forgejo's one-call review submit uses `APPROVED`, `REQUEST_CHANGES`, and
/// `COMMENT`. [`ReviewDecision::Pending`] has no safe one-call submit (the old
/// two-step pending flow drops the body for `APPROVED`), so it is rejected.
pub(crate) fn review_event_token(decision: ReviewDecision) -> ForgeResult<&'static str> {
    match decision {
        ReviewDecision::Approved => Ok("APPROVED"),
        ReviewDecision::ChangesRequested => Ok("REQUEST_CHANGES"),
        ReviewDecision::Commented => Ok("COMMENT"),
        ReviewDecision::Pending => Err(ForgeError::InvalidRequest(
            "forgejo backend cannot submit a pending review in one call; submit \
             approved, changes_requested, or commented instead"
                .to_string(),
        )),
    }
}

/// Returns the Forgejo merge `Do` token for a portable merge method.
pub(crate) fn merge_method_token(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::MergeCommit => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::format_pull_request_id;
    use temper_forge::{ItemNumber, UserId};

    fn repo() -> RepoCoord {
        RepoCoord::new("acme", "widgets")
    }

    #[test]
    fn maps_review_decisions_keeps_dismissed_and_filters_requests() {
        let pr_id = format_pull_request_id(&repo(), ItemNumber::new(7));
        let approved: ReviewDto = serde_json::from_str(
            r#"{"id": 1, "user": {"login": "carol"}, "state": "APPROVED", "submitted_at": "2024-03-03T00:00:00Z"}"#,
        )
        .unwrap();
        let review = map_review(&repo(), &pr_id, approved).unwrap();
        assert_eq!(review.decision, ReviewDecision::Approved);
        assert_eq!(review.reviewer_id, UserId::new("carol"));

        // A dismissed/stale verdict is **kept**: Forgejo auto-dismisses a prior
        // review when the same reviewer resubmits, and dropping it would erase the
        // changes-requested event from history. The reference backends keep every
        // verdict, and the portable aggregate resolves superseding by latest.
        let dismissed: ReviewDto = serde_json::from_str(
            r#"{"id": 2, "user": {"login": "carol"}, "state": "REQUEST_CHANGES", "submitted_at": "2024-03-03T00:00:00Z", "dismissed": true, "stale": true}"#,
        )
        .unwrap();
        let kept = map_review(&repo(), &pr_id, dismissed).expect("dismissed review is kept");
        assert_eq!(kept.decision, ReviewDecision::ChangesRequested);

        // Non-verdict events (review requests) are still dropped.
        let request: ReviewDto = serde_json::from_str(
            r#"{"id": 3, "user": {"login": "carol"}, "state": "REQUEST_REVIEW", "created_at": "2024-03-03T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(map_review(&repo(), &pr_id, request).is_none());
    }

    #[test]
    fn review_decision_accepts_event_and_state_names() {
        assert_eq!(
            map_review_decision("changes_requested"),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(
            map_review_decision("REQUEST_CHANGES"),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(
            map_review_decision("Commented"),
            Some(ReviewDecision::Commented)
        );
        assert_eq!(
            map_review_decision("pending"),
            Some(ReviewDecision::Pending)
        );
        assert_eq!(map_review_decision("request_review"), None);
    }

    #[test]
    fn review_event_token_maps_and_rejects_pending() {
        assert_eq!(
            review_event_token(ReviewDecision::Approved).unwrap(),
            "APPROVED"
        );
        assert_eq!(
            review_event_token(ReviewDecision::ChangesRequested).unwrap(),
            "REQUEST_CHANGES"
        );
        assert_eq!(
            review_event_token(ReviewDecision::Commented).unwrap(),
            "COMMENT"
        );
        assert!(matches!(
            review_event_token(ReviewDecision::Pending),
            Err(ForgeError::InvalidRequest(_))
        ));
    }

    #[test]
    fn merge_method_tokens() {
        assert_eq!(merge_method_token(MergeMethod::MergeCommit), "merge");
        assert_eq!(merge_method_token(MergeMethod::Squash), "squash");
        assert_eq!(merge_method_token(MergeMethod::Rebase), "rebase");
    }
}
