//! Mapping for GitHub pull-request reviews into the portable
//! [`PullRequestReview`] model, plus the review decision/event tokens.

use super::normalize;
use crate::ids::{RepoCoord, format_review_id, format_user_id};
use crate::types::ReviewDto;
use temper_forge::{ForgeError, ForgeResult, PullRequestId, PullRequestReview, ReviewDecision};

/// Maps a GitHub review DTO into a portable [`PullRequestReview`].
///
/// Returns `None` for states without a portable decision: GitHub's `DISMISSED`
/// state replaces the original verdict (unlike Forgejo, which keeps the verdict
/// and flags it), so a dismissed review carries no decision to report and is
/// dropped. `PENDING` (an unsubmitted draft visible only to its author) maps to
/// the portable `Pending` decision when it carries a timestamp.
pub(crate) fn map_review(
    repo: &RepoCoord,
    pull_request_id: &PullRequestId,
    dto: ReviewDto,
) -> Option<PullRequestReview> {
    let decision = map_review_decision(&dto.state)?;
    let submitted_at = dto.submitted_at?;
    Some(PullRequestReview {
        id: format_review_id(repo, dto.id),
        pull_request_id: pull_request_id.clone(),
        reviewer_id: format_user_id(&dto.user.login),
        decision,
        body: dto.body,
        submitted_at,
    })
}

/// Maps a GitHub review state string to a portable [`ReviewDecision`].
///
/// Accepts both GitHub's stored state names and its submit event names.
/// Returns `None` for dismissed and unknown states.
pub(crate) fn map_review_decision(state: &str) -> Option<ReviewDecision> {
    match normalize(state).as_str() {
        "approved" | "approve" => Some(ReviewDecision::Approved),
        "changes_requested" | "request_changes" => Some(ReviewDecision::ChangesRequested),
        "commented" | "comment" => Some(ReviewDecision::Commented),
        "pending" => Some(ReviewDecision::Pending),
        _ => None,
    }
}

/// Returns the GitHub submit event token for a portable review decision.
///
/// GitHub's one-call review submit uses `APPROVE`, `REQUEST_CHANGES`, and
/// `COMMENT`. [`ReviewDecision::Pending`] has no one-call submit (omitting the
/// event leaves a draft the gate cannot observe), so it is rejected.
pub(crate) fn review_event_token(decision: ReviewDecision) -> ForgeResult<&'static str> {
    match decision {
        ReviewDecision::Approved => Ok("APPROVE"),
        ReviewDecision::ChangesRequested => Ok("REQUEST_CHANGES"),
        ReviewDecision::Commented => Ok("COMMENT"),
        ReviewDecision::Pending => Err(ForgeError::InvalidRequest(
            "github backend cannot submit a pending review in one call; submit \
             approved, changes_requested, or commented instead"
                .to_string(),
        )),
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
    fn maps_review_decisions_and_drops_dismissed() {
        let pr_id = format_pull_request_id(&repo(), ItemNumber::new(7));
        let approved: ReviewDto = serde_json::from_str(
            r#"{"id": 1, "user": {"login": "carol"}, "state": "APPROVED", "submitted_at": "2024-03-03T00:00:00Z"}"#,
        )
        .unwrap();
        let review = map_review(&repo(), &pr_id, approved).unwrap();
        assert_eq!(review.decision, ReviewDecision::Approved);
        assert_eq!(review.reviewer_id, UserId::new("carol"));

        // GitHub's DISMISSED state replaces the original verdict, so there is no
        // decision left to report and the event is dropped.
        let dismissed: ReviewDto = serde_json::from_str(
            r#"{"id": 2, "user": {"login": "carol"}, "state": "DISMISSED", "submitted_at": "2024-03-03T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(map_review(&repo(), &pr_id, dismissed).is_none());

        // A pending draft without a timestamp cannot be ordered and is dropped.
        let pending: ReviewDto = serde_json::from_str(
            r#"{"id": 3, "user": {"login": "carol"}, "state": "PENDING", "submitted_at": null}"#,
        )
        .unwrap();
        assert!(map_review(&repo(), &pr_id, pending).is_none());
    }

    #[test]
    fn review_decision_accepts_event_and_state_names() {
        assert_eq!(
            map_review_decision("CHANGES_REQUESTED"),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(
            map_review_decision("request_changes"),
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
        assert_eq!(map_review_decision("DISMISSED"), None);
    }

    #[test]
    fn review_event_token_maps_and_rejects_pending() {
        assert_eq!(
            review_event_token(ReviewDecision::Approved).unwrap(),
            "APPROVE"
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
}
