//! Pure conversions from Forgejo DTOs into portable `temper_forge` models.
//!
//! These functions contain no HTTP or state; they translate the lenient DTOs in
//! [`crate::types`] into the domain types in [`temper_forge`]. Keeping mapping
//! pure makes the request/response plumbing in [`crate::pulls`] and
//! [`crate::items`] thin and lets the conversions be unit-tested directly.
//!
//! Determinism: labels, assignees, and requested reviewers are sorted and
//! deduplicated so two reads of the same artifact produce identical vectors,
//! matching the reference backends' contract (see ADR 0008).

use crate::ids::{
    RepoCoord, format_comment_id, format_issue_id, format_label_id, format_pull_request_id,
    format_repository_id, format_review_id, format_user_id,
};
use crate::types::{
    CommentDto, IssueDto, LabelDto, PrBranchDto, PrRepoDto, PullRequestDto, RepositoryDto,
    ReviewDto, UserDto,
};
use temper_forge::{
    BranchRef, Comment, ForgeError, ForgeResult, Issue, IssueState, ItemNumber, Label, MergeMethod,
    MergeRecord, PullRequest, PullRequestId, PullRequestReview, PullRequestState, Repository,
    ReviewDecision, User, UserId, Version,
};

/// Maps a Forgejo user DTO into a portable [`User`].
///
/// The Forgejo login is both the portable [`UserId`] and the human-facing
/// handle. Empty `full_name`/`email` strings (Forgejo's "unset" form) map to
/// `None`, matching the reference backends' optional fields.
pub(crate) fn map_user(dto: UserDto) -> User {
    User {
        id: format_user_id(&dto.login),
        handle: dto.login,
        display_name: non_empty(dto.full_name),
        email: non_empty(dto.email),
    }
}

/// Maps a Forgejo repository DTO into a portable [`Repository`].
pub(crate) fn map_repository(dto: RepositoryDto) -> Repository {
    let repo = RepoCoord::new(dto.owner.login, dto.name);
    Repository {
        id: format_repository_id(&repo),
        owner: repo.owner,
        name: repo.name,
        default_branch: dto.default_branch,
        description: non_empty(dto.description),
        created_at: dto.created_at,
        updated_at: dto.updated_at,
    }
}

/// Maps a Forgejo label DTO into a portable [`Label`] scoped to `repo`.
///
/// The numeric provider id becomes the prefixed opaque [`LabelId`]; empty
/// color/description strings map to `None`.
pub(crate) fn map_label(repo: &RepoCoord, dto: LabelDto) -> Label {
    Label {
        id: format_label_id(repo, dto.id),
        repo_id: format_repository_id(repo),
        name: dto.name,
        color: non_empty(dto.color),
        description: non_empty(dto.description),
    }
}

/// Returns `None` for a missing or empty string, else the value unchanged.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// Maps a Forgejo comment DTO into a portable [`Comment`].
pub(crate) fn map_comment(repo: &RepoCoord, dto: CommentDto) -> Comment {
    Comment {
        id: format_comment_id(repo, dto.id),
        author_id: format_user_id(&dto.user.login),
        body: dto.body,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
    }
}

/// Maps a Forgejo issue DTO into a portable [`Issue`].
///
/// `version` is set to [`Version::INITIAL`]; callers that have the response
/// validator overwrite it through the backend's version cache.
/// `dependencies` is left empty here and populated by the dependency-link
/// enrichment step (see [`crate::dependencies`]).
pub(crate) fn map_issue(repo: &RepoCoord, dto: IssueDto) -> Issue {
    let number = ItemNumber::new(dto.number);
    let state = if normalize(&dto.state) == "closed" {
        IssueState::Closed
    } else {
        IssueState::Open
    };
    let labels = sorted_dedup(
        dto.labels
            .unwrap_or_default()
            .into_iter()
            .map(|label| label.name)
            .collect(),
    );
    let assignees = sorted_dedup_users(map_logins(dto.assignees));

    Issue {
        id: format_issue_id(repo, number),
        repo_id: format_repository_id(repo),
        number,
        title: dto.title,
        body: dto.body,
        state,
        author_id: format_user_id(&dto.user.login),
        labels,
        assignees,
        dependencies: Vec::new(),
        version: Version::INITIAL,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
        closed_at: dto.closed_at,
    }
}

/// Maps a Forgejo pull-request DTO into a portable [`PullRequest`].
///
/// `version` is set to [`Version::INITIAL`] here; callers that have the response
/// validator overwrite it through the backend's version cache. `dependencies`
/// is left empty (the dependency phase fills it). Forgejo's pull-request JSON
/// does not expose the merge method, so a merged pull request maps
/// `MergeRecord::method` to [`MergeMethod::MergeCommit`] as a documented default;
/// `merge_pull_request` reports the method actually requested.
pub(crate) fn map_pull_request(repo: &RepoCoord, dto: PullRequestDto) -> PullRequest {
    let number = ItemNumber::new(dto.number);
    let id = format_pull_request_id(repo, number);
    let state = if dto.merged {
        PullRequestState::Merged
    } else if normalize(&dto.state) == "closed" {
        PullRequestState::Closed
    } else {
        PullRequestState::Open
    };

    let (source, head_sha) = branch_side(repo, dto.head);
    let (target, base_sha) = branch_side(repo, dto.base);

    let labels = sorted_dedup(
        dto.labels
            .unwrap_or_default()
            .into_iter()
            .map(|label| label.name)
            .collect(),
    );
    let assignees = sorted_dedup_users(map_logins(dto.assignees));
    let requested_reviewers = sorted_dedup_users(map_logins(dto.requested_reviewers));

    let merge = if dto.merged {
        dto.merge_commit_sha.clone().map(|sha| MergeRecord {
            method: MergeMethod::MergeCommit,
            commit_sha: sha,
            merged_by: dto
                .merged_by
                .as_ref()
                .map(|user| format_user_id(&user.login))
                .unwrap_or_else(|| format_user_id(&dto.user.login)),
            merged_at: dto.merged_at.unwrap_or(dto.updated_at),
        })
    } else {
        None
    };

    PullRequest {
        id,
        repo_id: format_repository_id(repo),
        number,
        title: dto.title,
        body: dto.body,
        state,
        author_id: format_user_id(&dto.user.login),
        source,
        target,
        head_sha,
        base_sha,
        labels,
        assignees,
        requested_reviewers,
        dependencies: Vec::new(),
        merge,
        version: Version::INITIAL,
        created_at: dto.created_at,
        updated_at: dto.updated_at,
        closed_at: dto.closed_at,
    }
}

/// Maps a Forgejo review DTO into a portable [`PullRequestReview`].
///
/// Returns `None` only for review-request events (`REQUEST_REVIEW`) and any state
/// without a portable decision (`PENDING`, comment-only on some versions).
///
/// **Dismissed and stale reviews are kept.** The reference (filesystem/memory)
/// backends have no dismissal concept and return every verdict event, and the
/// portable review aggregate ([`PullRequestReviewStatus::from_reviews`]) already
/// resolves superseding by taking the **latest review per reviewer**. Forgejo,
/// however, auto-dismisses a reviewer's prior review when they submit a new one
/// (e.g. an approval after a changes-requested review), so dropping dismissed
/// reviews here would erase the changes-requested event from history and diverge
/// from the reference contract — breaking history-sensitive consumers while not
/// affecting the gate (the approval is still the latest). Keeping them aligns the
/// backends; see `docs/reference/forge-interface.md`.
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

/// Builds a branch reference plus its commit SHA from a head/base DTO side.
fn branch_side(pr_repo: &RepoCoord, branch: Option<PrBranchDto>) -> (BranchRef, Option<String>) {
    let dto = branch.unwrap_or_default();
    let sha = dto.sha.filter(|sha| !sha.is_empty());
    let repo_coord = dto
        .repo
        .and_then(repo_coord_from_dto)
        .unwrap_or_else(|| pr_repo.clone());
    let branch_name = dto
        .branch
        .filter(|branch| !branch.is_empty())
        .or_else(|| dto.label.map(|label| strip_owner_prefix(&label)))
        .unwrap_or_default();
    (
        BranchRef {
            repository_id: format_repository_id(&repo_coord),
            branch: branch_name,
        },
        sha,
    )
}

/// Resolves a repository coordinate from an embedded branch repo DTO.
fn repo_coord_from_dto(dto: PrRepoDto) -> Option<RepoCoord> {
    if let Some(full_name) = dto.full_name.filter(|name| !name.is_empty())
        && let Some((owner, name)) = full_name.split_once('/')
        && !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
    {
        return Some(RepoCoord::new(owner, name));
    }
    let owner = dto.owner.map(|user| user.login).filter(|o| !o.is_empty())?;
    let name = dto.name.filter(|name| !name.is_empty())?;
    Some(RepoCoord::new(owner, name))
}

/// Strips a `owner:` prefix from a branch label, leaving the branch name.
fn strip_owner_prefix(label: &str) -> String {
    label
        .split_once(':')
        .map(|(_, branch)| branch.to_string())
        .unwrap_or_else(|| label.to_string())
}

/// Normalizes a provider state string: lowercase, spaces/dashes to underscores.
fn normalize(value: &str) -> String {
    value.trim().to_lowercase().replace([' ', '-'], "_")
}

fn map_logins(users: Option<Vec<crate::types::UserDto>>) -> Vec<UserId> {
    users
        .unwrap_or_default()
        .into_iter()
        .map(|user| format_user_id(&user.login))
        .collect()
}

fn sorted_dedup(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn sorted_dedup_users(mut values: Vec<UserId>) -> Vec<UserId> {
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::parse_pull_request_id;

    fn repo() -> RepoCoord {
        RepoCoord::new("acme", "widgets")
    }

    fn pull_request_dto(json: &str) -> PullRequestDto {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn maps_open_pull_request_with_branches_and_sorts_collections() {
        let pr = map_pull_request(
            &repo(),
            pull_request_dto(
                r#"{
                    "number": 42,
                    "title": "Add widget",
                    "body": "details",
                    "state": "open",
                    "merged": false,
                    "user": {"login": "author"},
                    "head": {"label": "author:feature", "ref": "feature", "sha": "headsha"},
                    "base": {"ref": "main", "sha": "basesha"},
                    "labels": [{"id": 2, "name": "ready"}, {"id": 1, "name": "needs-ci"}],
                    "assignees": [{"login": "carol"}, {"login": "bob"}],
                    "requested_reviewers": [{"login": "dave"}],
                    "created_at": "2024-03-01T00:00:00Z",
                    "updated_at": "2024-03-02T00:00:00Z"
                }"#,
            ),
        );

        assert_eq!(pr.id, format_pull_request_id(&repo(), ItemNumber::new(42)));
        assert_eq!(
            parse_pull_request_id(&pr.id).unwrap().1,
            ItemNumber::new(42)
        );
        assert_eq!(pr.state, PullRequestState::Open);
        assert_eq!(pr.author_id, UserId::new("author"));
        assert_eq!(pr.source.branch, "feature");
        assert_eq!(pr.target.branch, "main");
        assert_eq!(pr.head_sha.as_deref(), Some("headsha"));
        assert_eq!(pr.base_sha.as_deref(), Some("basesha"));
        assert_eq!(pr.labels, vec!["needs-ci".to_string(), "ready".to_string()]);
        assert_eq!(pr.assignees, vec![UserId::new("bob"), UserId::new("carol")]);
        assert_eq!(pr.requested_reviewers, vec![UserId::new("dave")]);
        assert!(pr.merge.is_none());
        assert_eq!(pr.version, Version::INITIAL);
    }

    #[test]
    fn maps_issue_and_sorts_collections() {
        let issue: IssueDto = serde_json::from_str(
            r#"{
                "number": 7,
                "title": "Fix bug",
                "body": "details",
                "state": "open",
                "user": {"login": "author"},
                "labels": [{"id": 2, "name": "ready"}, {"id": 1, "name": "bug"}],
                "assignees": [{"login": "carol"}, {"login": "bob"}],
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            }"#,
        )
        .unwrap();
        let mapped = map_issue(&repo(), issue);
        assert_eq!(mapped.id, format_issue_id(&repo(), ItemNumber::new(7)));
        assert_eq!(mapped.state, IssueState::Open);
        assert_eq!(mapped.author_id, UserId::new("author"));
        assert_eq!(mapped.labels, vec!["bug".to_string(), "ready".to_string()]);
        assert_eq!(
            mapped.assignees,
            vec![UserId::new("bob"), UserId::new("carol")]
        );
        assert!(mapped.dependencies.is_empty());
        assert_eq!(mapped.version, Version::INITIAL);

        let closed: IssueDto = serde_json::from_str(
            r#"{
                "number": 8,
                "state": "closed",
                "user": {"login": "author"},
                "labels": null,
                "assignees": null,
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z",
                "closed_at": "2024-03-03T00:00:00Z"
            }"#,
        )
        .unwrap();
        let mapped = map_issue(&repo(), closed);
        assert_eq!(mapped.state, IssueState::Closed);
        assert!(mapped.labels.is_empty());
        assert!(mapped.closed_at.is_some());
    }

    #[test]
    fn maps_merged_pull_request_to_merge_record() {
        let pr = map_pull_request(
            &repo(),
            pull_request_dto(
                r#"{
                    "number": 7,
                    "title": "merged",
                    "state": "closed",
                    "merged": true,
                    "merged_at": "2024-04-01T00:00:00Z",
                    "merge_commit_sha": "mergesha",
                    "merged_by": {"login": "maintainer"},
                    "user": {"login": "author"},
                    "created_at": "2024-03-01T00:00:00Z",
                    "updated_at": "2024-04-01T00:00:00Z",
                    "closed_at": "2024-04-01T00:00:00Z"
                }"#,
            ),
        );

        assert_eq!(pr.state, PullRequestState::Merged);
        let merge = pr.merge.expect("merged pull request has a merge record");
        assert_eq!(merge.commit_sha, "mergesha");
        assert_eq!(merge.merged_by, UserId::new("maintainer"));
        assert_eq!(merge.method, MergeMethod::MergeCommit);
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

    #[test]
    fn branch_uses_head_repo_when_present() {
        let pr = map_pull_request(
            &repo(),
            pull_request_dto(
                r#"{
                    "number": 9,
                    "state": "open",
                    "user": {"login": "author"},
                    "head": {"ref": "fork-branch", "repo": {"full_name": "forker/widgets"}},
                    "base": {"ref": "main"},
                    "created_at": "2024-03-01T00:00:00Z",
                    "updated_at": "2024-03-02T00:00:00Z"
                }"#,
            ),
        );
        assert_eq!(
            pr.source.repository_id,
            format_repository_id(&RepoCoord::new("forker", "widgets"))
        );
        assert_eq!(pr.target.repository_id, format_repository_id(&repo()));
    }
}
