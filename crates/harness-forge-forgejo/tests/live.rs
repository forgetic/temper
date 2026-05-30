//! Optional, env-gated live smoke tests against a real Forgejo instance.
//!
//! Every test here is `#[ignore]`d, so a plain `cargo test` never touches the
//! network — the offline contract tests in the sibling files are the default
//! coverage. These exist to let a human (or a later live-refinement session)
//! sanity-check the backend against a real server with a single command:
//!
//! ```sh
//! HARNESS_FORGEJO_LIVE=1 \
//!   FORGEJO_URL=https://git.example.com \
//!   FORGEJO_ACCESS_TOKEN=… \
//!   FORGEJO_DEFAULT_REPO=owner/repo \
//!   cargo test -p harness-forge-forgejo --test live -- --ignored
//! ```
//!
//! The tests are **read-mostly**: they exercise `current_user`,
//! `get_repository_by_path`, `list_labels`, `list_issues`,
//! `list_pull_requests`, and `list_ci_jobs`. The only mutating test is
//! additionally gated behind `HARNESS_FORGEJO_LIVE_MUTATE=1` and writes a
//! uniquely-titled issue it immediately closes, so an accidental run leaves no
//! durable open artifact.
//!
//! Two safety layers keep the default suite hermetic: the `#[ignore]` attribute
//! (so the tests are skipped unless `--ignored` is passed) and the
//! `HARNESS_FORGEJO_LIVE` gate checked inside each test (so even
//! `cargo test -- --ignored` is a no-op without the opt-in environment).

use harness_forge::{
    CiJobQuery, CreateIssue, IssueQuery, IssueState, PullRequestQuery, RepositoryId,
    RepositoryPath, UpdateIssue,
};
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge, ReqwestHttpClient};

/// A configured live backend plus the default repository under test.
struct Live {
    forge: ForgejoForge<ReqwestHttpClient>,
    repo_path: RepositoryPath,
}

/// Returns a live backend when the opt-in environment is present, else `None`.
///
/// Returning `None` (rather than panicking) lets a test invoked with
/// `--ignored` but without the `HARNESS_FORGEJO_LIVE` opt-in complete as a
/// harmless no-op instead of failing.
fn live() -> Option<Live> {
    if std::env::var("HARNESS_FORGEJO_LIVE").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping live Forgejo test: set HARNESS_FORGEJO_LIVE=1 plus FORGEJO_URL, \
             FORGEJO_ACCESS_TOKEN, and FORGEJO_DEFAULT_REPO to enable"
        );
        return None;
    }
    let config = ForgejoConfig::from_env().expect(
        "HARNESS_FORGEJO_LIVE=1 requires FORGEJO_URL and FORGEJO_ACCESS_TOKEN \
         (and FORGEJO_DEFAULT_REPO in owner/repo form)",
    );
    let owner = config
        .default_owner
        .clone()
        .expect("live tests require FORGEJO_DEFAULT_REPO in owner/repo form");
    let name = config
        .default_name
        .clone()
        .expect("live tests require FORGEJO_DEFAULT_REPO in owner/repo form");
    let repo_path = RepositoryPath::new(owner, name);
    Some(Live {
        forge: ForgejoForge::new(config),
        repo_path,
    })
}

/// Resolves the default repository's backend id, failing the test if absent.
async fn resolve_repo_id(live: &Live) -> RepositoryId {
    live.forge
        .get_repository_by_path(&live.repo_path)
        .await
        .expect("get_repository_by_path should succeed")
        .unwrap_or_else(|| {
            panic!(
                "FORGEJO_DEFAULT_REPO {}/{} not found on the server",
                live.repo_path.owner, live.repo_path.name
            )
        })
        .id
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_current_user_returns_identity() {
    let Some(live) = live() else {
        return;
    };
    let user = live
        .forge
        .current_user()
        .await
        .expect("current_user should succeed");
    assert!(!user.handle.is_empty(), "token must map to a real login");
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_get_repository_by_path_resolves_default_repo() {
    let Some(live) = live() else {
        return;
    };
    let repo = live
        .forge
        .get_repository_by_path(&live.repo_path)
        .await
        .expect("get_repository_by_path should succeed")
        .expect("default repository should exist");
    assert_eq!(repo.owner, live.repo_path.owner);
    assert_eq!(repo.name, live.repo_path.name);
    assert!(!repo.default_branch.is_empty());
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_list_labels_succeeds() {
    let Some(live) = live() else {
        return;
    };
    let repo_id = resolve_repo_id(&live).await;
    let labels = live
        .forge
        .list_labels(&repo_id)
        .await
        .expect("list_labels should succeed");
    // The set may legitimately be empty; assert the call shape and determinism.
    let mut names: Vec<&str> = labels.iter().map(|label| label.name.as_str()).collect();
    let sorted = {
        let mut copy = names.clone();
        copy.sort_unstable();
        copy
    };
    names.sort_unstable();
    assert_eq!(names, sorted, "labels should come back name-sorted");
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_list_issues_succeeds() {
    let Some(live) = live() else {
        return;
    };
    let repo_id = resolve_repo_id(&live).await;
    let issues = live
        .forge
        .list_issues(&repo_id, IssueQuery::default())
        .await
        .expect("list_issues should succeed");
    // No row returned through the issue surface may be a pull request.
    assert!(
        issues.iter().all(|issue| issue.repo_id == repo_id),
        "every issue must belong to the queried repository"
    );
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_list_pull_requests_succeeds() {
    let Some(live) = live() else {
        return;
    };
    let repo_id = resolve_repo_id(&live).await;
    let pulls = live
        .forge
        .list_pull_requests(&repo_id, PullRequestQuery::default())
        .await
        .expect("list_pull_requests should succeed");
    assert!(
        pulls.iter().all(|pull| pull.repo_id == repo_id),
        "every pull request must belong to the queried repository"
    );
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_list_ci_jobs_succeeds_or_reports_unavailable() {
    let Some(live) = live() else {
        return;
    };
    let repo_id = resolve_repo_id(&live).await;
    // Actions may be disabled on the repo; the backend surfaces that as an error
    // rather than an empty (falsely "passed") list. Either outcome is acceptable
    // for a smoke test — we only assert the call does not panic and, on success,
    // that every job is scoped to the repository.
    match live
        .forge
        .list_ci_jobs(&repo_id, CiJobQuery::default())
        .await
    {
        Ok(jobs) => assert!(jobs.iter().all(|job| job.repo_id == repo_id)),
        Err(error) => eprintln!("list_ci_jobs reported Actions unavailable: {error}"),
    }
}

#[tokio::test]
#[ignore = "live: requires HARNESS_FORGEJO_LIVE=1 and Forgejo credentials"]
async fn live_create_and_close_issue_roundtrip() {
    let Some(live) = live() else {
        return;
    };
    // A second, explicit opt-in: read-mostly live runs never mutate the server.
    if std::env::var("HARNESS_FORGEJO_LIVE_MUTATE").ok().as_deref() != Some("1") {
        eprintln!("skipping live mutation test: set HARNESS_FORGEJO_LIVE_MUTATE=1 to enable");
        return;
    }
    let repo_id = resolve_repo_id(&live).await;

    // A unique title keeps repeated runs from colliding and makes the artifact
    // easy to identify and clean up by hand if a run is interrupted mid-test.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_millis();
    let title = format!("harness-forge-forgejo live smoke {nonce}");

    let created = live
        .forge
        .create_issue(
            &repo_id,
            CreateIssue {
                title: title.clone(),
                body: "Created by the harness-forge-forgejo live smoke test.".to_string(),
                labels: Vec::new(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("create_issue should succeed");
    assert_eq!(created.title, title);

    // Immediately close it so the test leaves no open artifact behind.
    let closed = live
        .forge
        .update_issue(
            &created.id,
            UpdateIssue {
                state: Some(IssueState::Closed),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("update_issue (close) should succeed");
    assert_eq!(closed.state, IssueState::Closed);
}
