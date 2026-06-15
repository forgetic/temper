//! Conversions for pull-request DTOs, including the head/base branch sides.

use super::{map_logins, normalize, sorted_dedup, sorted_dedup_users};
use crate::ids::{RepoCoord, format_pull_request_id, format_repository_id, format_user_id};
use crate::types::{PrBranchDto, PrRepoDto, PullRequestDto};
use temper_forge::{
    BranchRef, ItemNumber, MergeMethod, MergeRecord, PullRequest, PullRequestState, Version,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::parse_pull_request_id;
    use temper_forge::UserId;

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
