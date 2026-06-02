//! Backend-owned identifier encoding and parsing.
//!
//! These encodings are private to the Forgejo backend. Workflow code must treat
//! the resulting ids as opaque and never parse them; only this module maps
//! between portable [`temper_forge`] id newtypes and Forgejo coordinates.
//!
//! Shapes (see `forgejo-00-plan.md`):
//!
//! - repository: `forgejo:{owner}/{repo}`
//! - issue: `forgejo:{owner}/{repo}:issue:{number}`
//! - pull request: `forgejo:{owner}/{repo}:pull:{number}`
//! - comment: `forgejo:{owner}/{repo}:comment:{id}`
//! - review: `forgejo:{owner}/{repo}:review:{id}`
//! - label: `forgejo:{owner}/{repo}:label:{id}`
//! - CI job: `forgejo:{owner}/{repo}:actions:{run}:{job_index}:{task_id}`
//! - user: the Forgejo login, unprefixed
//!
//! Owner and repository names cannot contain `:` or `/` on Forgejo, so splitting
//! on those characters is unambiguous.

use temper_forge::{
    CiJobId, CommentId, ForgeError, ForgeResult, IssueId, ItemNumber, LabelId, PullRequestId,
    RepositoryId, ReviewId, UserId,
};

/// Identifier scheme prefix owned by this backend.
const SCHEME: &str = "forgejo";

/// Owner/name coordinate for a Forgejo repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepoCoord {
    pub owner: String,
    pub name: String,
}

impl RepoCoord {
    pub(crate) fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }

    /// Returns the `owner/repo` path segment used in API URLs and ids.
    pub(crate) fn path_segment(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Decoded coordinate for a Forgejo Actions CI job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CiJobCoord {
    pub repo: RepoCoord,
    pub run: u64,
    pub job_index: u64,
    pub task_id: u64,
}

fn invalid_id(kind: &str, raw: &str) -> ForgeError {
    ForgeError::InvalidRequest(format!("not a forgejo {kind} id: {raw:?}"))
}

/// Parses an `owner/repo` segment, rejecting empty or multi-slash values.
fn parse_repo_segment(segment: &str, kind: &str, raw: &str) -> ForgeResult<RepoCoord> {
    let (owner, name) = segment
        .split_once('/')
        .ok_or_else(|| invalid_id(kind, raw))?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(invalid_id(kind, raw));
    }
    Ok(RepoCoord::new(owner, name))
}

/// Parses a `forgejo:{owner}/{repo}:{tag}:{number}` id.
fn parse_numbered(kind: &str, tag: &str, raw: &str) -> ForgeResult<(RepoCoord, u64)> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 4 || parts[0] != SCHEME || parts[2] != tag {
        return Err(invalid_id(kind, raw));
    }
    let repo = parse_repo_segment(parts[1], kind, raw)?;
    let number = parts[3].parse::<u64>().map_err(|_| invalid_id(kind, raw))?;
    Ok((repo, number))
}

// --- repository -------------------------------------------------------------

pub(crate) fn format_repository_id(repo: &RepoCoord) -> RepositoryId {
    RepositoryId::new(format!("{SCHEME}:{}", repo.path_segment()))
}

pub(crate) fn parse_repository_id(id: &RepositoryId) -> ForgeResult<RepoCoord> {
    let raw = id.as_str();
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 2 || parts[0] != SCHEME {
        return Err(invalid_id("repository", raw));
    }
    parse_repo_segment(parts[1], "repository", raw)
}

// --- issue / pull request ----------------------------------------------------

pub(crate) fn format_issue_id(repo: &RepoCoord, number: ItemNumber) -> IssueId {
    IssueId::new(format!(
        "{SCHEME}:{}:issue:{}",
        repo.path_segment(),
        number.get()
    ))
}

pub(crate) fn parse_issue_id(id: &IssueId) -> ForgeResult<(RepoCoord, ItemNumber)> {
    parse_numbered("issue", "issue", id.as_str()).map(|(repo, n)| (repo, ItemNumber::new(n)))
}

pub(crate) fn format_pull_request_id(repo: &RepoCoord, number: ItemNumber) -> PullRequestId {
    PullRequestId::new(format!(
        "{SCHEME}:{}:pull:{}",
        repo.path_segment(),
        number.get()
    ))
}

pub(crate) fn parse_pull_request_id(id: &PullRequestId) -> ForgeResult<(RepoCoord, ItemNumber)> {
    parse_numbered("pull request", "pull", id.as_str()).map(|(repo, n)| (repo, ItemNumber::new(n)))
}

// --- comment / review / label ------------------------------------------------

pub(crate) fn format_comment_id(repo: &RepoCoord, provider_id: u64) -> CommentId {
    CommentId::new(format!(
        "{SCHEME}:{}:comment:{provider_id}",
        repo.path_segment()
    ))
}

// The comment/review/label id parsers complete the symmetric encode/parse API
// and are covered by the round-trip unit tests below, but the `Forge` trait
// exposes no get-by-id for these artifacts, so production never calls them.
#[allow(dead_code)]
pub(crate) fn parse_comment_id(id: &CommentId) -> ForgeResult<(RepoCoord, u64)> {
    parse_numbered("comment", "comment", id.as_str())
}

pub(crate) fn format_review_id(repo: &RepoCoord, provider_id: u64) -> ReviewId {
    ReviewId::new(format!(
        "{SCHEME}:{}:review:{provider_id}",
        repo.path_segment()
    ))
}

#[allow(dead_code)]
pub(crate) fn parse_review_id(id: &ReviewId) -> ForgeResult<(RepoCoord, u64)> {
    parse_numbered("review", "review", id.as_str())
}

pub(crate) fn format_label_id(repo: &RepoCoord, provider_id: u64) -> LabelId {
    LabelId::new(format!(
        "{SCHEME}:{}:label:{provider_id}",
        repo.path_segment()
    ))
}

#[allow(dead_code)]
pub(crate) fn parse_label_id(id: &LabelId) -> ForgeResult<(RepoCoord, u64)> {
    parse_numbered("label", "label", id.as_str())
}

// --- CI job ------------------------------------------------------------------

pub(crate) fn format_ci_job_id(coord: &CiJobCoord) -> CiJobId {
    CiJobId::new(format!(
        "{SCHEME}:{}:actions:{}:{}:{}",
        coord.repo.path_segment(),
        coord.run,
        coord.job_index,
        coord.task_id
    ))
}

pub(crate) fn parse_ci_job_id(id: &CiJobId) -> ForgeResult<CiJobCoord> {
    let raw = id.as_str();
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 6 || parts[0] != SCHEME || parts[2] != "actions" {
        return Err(invalid_id("ci job", raw));
    }
    let repo = parse_repo_segment(parts[1], "ci job", raw)?;
    let run = parts[3]
        .parse::<u64>()
        .map_err(|_| invalid_id("ci job", raw))?;
    let job_index = parts[4]
        .parse::<u64>()
        .map_err(|_| invalid_id("ci job", raw))?;
    let task_id = parts[5]
        .parse::<u64>()
        .map_err(|_| invalid_id("ci job", raw))?;
    Ok(CiJobCoord {
        repo,
        run,
        job_index,
        task_id,
    })
}

// --- user --------------------------------------------------------------------

/// Encodes a Forgejo login as a portable [`UserId`].
pub(crate) fn format_user_id(login: &str) -> UserId {
    UserId::new(login)
}

/// Decodes a [`UserId`] back into a Forgejo login, rejecting empty ids.
pub(crate) fn user_login(id: &UserId) -> ForgeResult<&str> {
    let login = id.as_str();
    if login.is_empty() {
        return Err(invalid_id("user", login));
    }
    Ok(login)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoCoord {
        RepoCoord::new("acme", "widgets")
    }

    #[test]
    fn repository_id_round_trips() {
        let id = format_repository_id(&repo());
        assert_eq!(id.as_str(), "forgejo:acme/widgets");
        assert_eq!(parse_repository_id(&id).unwrap(), repo());
    }

    #[test]
    fn issue_and_pull_ids_round_trip() {
        let issue = format_issue_id(&repo(), ItemNumber::new(7));
        assert_eq!(issue.as_str(), "forgejo:acme/widgets:issue:7");
        let (parsed_repo, number) = parse_issue_id(&issue).unwrap();
        assert_eq!(parsed_repo, repo());
        assert_eq!(number, ItemNumber::new(7));

        let pull = format_pull_request_id(&repo(), ItemNumber::new(42));
        assert_eq!(pull.as_str(), "forgejo:acme/widgets:pull:42");
        let (parsed_repo, number) = parse_pull_request_id(&pull).unwrap();
        assert_eq!(parsed_repo, repo());
        assert_eq!(number, ItemNumber::new(42));
    }

    #[test]
    fn comment_review_label_ids_round_trip() {
        let comment = format_comment_id(&repo(), 100);
        assert_eq!(parse_comment_id(&comment).unwrap(), (repo(), 100));

        let review = format_review_id(&repo(), 200);
        assert_eq!(parse_review_id(&review).unwrap(), (repo(), 200));

        let label = format_label_id(&repo(), 5);
        assert_eq!(parse_label_id(&label).unwrap(), (repo(), 5));
    }

    #[test]
    fn ci_job_id_round_trips() {
        let coord = CiJobCoord {
            repo: repo(),
            run: 12,
            job_index: 1,
            task_id: 345,
        };
        let id = format_ci_job_id(&coord);
        assert_eq!(id.as_str(), "forgejo:acme/widgets:actions:12:1:345");
        assert_eq!(parse_ci_job_id(&id).unwrap(), coord);
    }

    #[test]
    fn user_id_round_trips() {
        let id = format_user_id("octocat");
        assert_eq!(id.as_str(), "octocat");
        assert_eq!(user_login(&id).unwrap(), "octocat");
        assert!(user_login(&UserId::new("")).is_err());
    }

    #[test]
    fn rejects_foreign_shapes() {
        assert!(parse_repository_id(&RepositoryId::new("repo-0001")).is_err());
        assert!(parse_repository_id(&RepositoryId::new("forgejo:noslash")).is_err());
        assert!(parse_issue_id(&IssueId::new("forgejo:acme/widgets:pull:7")).is_err());
        assert!(parse_issue_id(&IssueId::new("forgejo:acme/widgets:issue:abc")).is_err());
        assert!(
            parse_pull_request_id(&PullRequestId::new("forgejo:acme/widgets:issue:7")).is_err()
        );
        assert!(parse_label_id(&LabelId::new("label-acme-widgets")).is_err());
        assert!(parse_ci_job_id(&CiJobId::new("forgejo:acme/widgets:actions:1:2")).is_err());
        assert!(parse_repository_id(&RepositoryId::new("forgejo:/widgets")).is_err());
    }
}
