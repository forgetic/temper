//! Phase 2b PR-prep test against a real Forgejo.
//!
//! `#[ignore]`d **and** gated behind `HARNESS_FORGEJO_E2E=1`, so the default
//! `cargo test` never downloads a binary, opens a socket, or spawns a server —
//! exactly like the Phase 1/1b/2 smoke tests. Run it with:
//!
//! ```sh
//! HARNESS_FORGEJO_E2E=1 \
//!   cargo test -p harness-testing --test forgejo_pr_prep -- --ignored
//! ```
//!
//! It boots a [`ForgejoServer`], provisions (Phase 2), then proves the Phase 2b
//! contract end to end against the real backend (findings-phase-0 §1):
//!
//! - the fake engineer's `CreatePullRequest` (head `fake/pr-for-code-{N}`, base
//!   `main`) **cannot** be opened as-is — `create_pull_request` 404s because the
//!   head branch is not a real ref;
//! - [`prepare_pull_request_head`] creates the head branch + a trivial differing
//!   commit, after which `create_pull_request` against the real server succeeds
//!   and the PR reads back `mergeable: true`;
//! - re-running the prep is a no-op (idempotent), so a re-attempted worker tick
//!   does not error.
//!
//! `#[tokio::test]` because `ForgejoForge` and the prep helper are async (a real
//! HTTP reactor).

use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};
use harness_testing::forgejo_server::{prepare_pull_request_head, provision, ForgejoServer};
use harness_testing::pull_request_input;
use harness_workflow::RoleId;
use serde_json::Value;

/// Returns whether the env opt-in is present; prints a skip note when not.
fn enabled() -> bool {
    if std::env::var("HARNESS_FORGEJO_E2E").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!(
        "skipping Forgejo PR-prep e2e test: set HARNESS_FORGEJO_E2E=1 to enable \
         (downloads a pinned Forgejo binary and boots a throwaway server)"
    );
    false
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots a real Forgejo server; run with HARNESS_FORGEJO_E2E=1 -- --ignored"]
async fn prep_makes_head_real_and_pr_is_mergeable() {
    if !enabled() {
        return;
    }

    // `ForgejoServer::start` uses a *blocking* reqwest client for readiness; boot
    // it off-reactor so its nested blocking runtime lives and dies off the async
    // test thread (same pattern as the Phase 2 provisioning test).
    let server = tokio::task::spawn_blocking(ForgejoServer::start)
        .await
        .expect("server boot task joins")
        .expect("forgejo server boots");
    let base = server.base_url().to_string();

    let provisioned = provision(&server).await.expect("provisioning succeeds");
    let engineer = provisioned
        .role(&RoleId::new("engineer"))
        .expect("engineer role is provisioned");

    // The engineer acts with its own token (Forgejo identity is the token). This
    // is the exact handle a `--backend forgejo` engineer worker would build.
    let forge = ForgejoForge::new(
        ForgejoConfig::new(&base, &engineer.token)
            .with_default_repo(&provisioned.owner, &provisioned.name),
    );

    // Build the same CreatePullRequest the fake engineer builds: head
    // `fake/pr-for-code-{N}`, base `main`, the `implementation` label.
    let code_number = 1u64;
    let head = format!("fake/pr-for-code-{code_number}");
    let input = pull_request_input(
        &provisioned.repository,
        format!("Implement #{code_number}: prep smoke"),
        format!("Fake implementation for code issue #{code_number}."),
        head.clone(),
        vec!["implementation".to_string()],
    );

    // 1. Without prep, the head is not a real ref → create_pull_request 404s.
    let unprepared = forge
        .create_pull_request(&provisioned.repository, input.clone())
        .await;
    assert!(
        matches!(unprepared, Err(harness_forge::ForgeError::NotFound(_))),
        "expected NotFound for a non-existent head branch, got {unprepared:?}"
    );

    // 2. Prep creates the head branch + a trivial differing commit.
    prepare_pull_request_head(
        &base,
        &engineer.token,
        &provisioned.owner,
        &provisioned.name,
        &input,
    )
    .await
    .expect("pr-prep creates head branch + commit");

    // The head branch now exists as a real ref.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("client builds");
    let branch_resp = client
        .get(format!(
            "{base}/api/v1/repos/{}/{}/branches/{head}",
            provisioned.owner, provisioned.name
        ))
        .header("Authorization", format!("token {}", engineer.token))
        .send()
        .await
        .expect("branch lookup sends");
    assert!(
        branch_resp.status().is_success(),
        "head branch should exist after prep, got {}",
        branch_resp.status()
    );

    // 3. create_pull_request now succeeds against the real server.
    let pull = forge
        .create_pull_request(&provisioned.repository, input.clone())
        .await
        .expect("create_pull_request succeeds once the head is real");

    // …and becomes mergeable (a non-empty diff against base, no conflicts). The
    // portable model does not surface mergeability, so confirm via raw REST.
    // Forgejo computes `mergeable` asynchronously after creation, so it is often
    // `false` for the first moment; poll until the background merge-check settles.
    let pr_url = format!(
        "{base}/api/v1/repos/{}/{}/pulls/{}",
        provisioned.owner,
        provisioned.name,
        pull.number.get()
    );
    let mergeable = poll_mergeable(&client, &pr_url, &engineer.token).await;
    assert!(
        mergeable,
        "PR should become mergeable after prep (head diverges from base)"
    );

    // 4. Re-running prep is a no-op: the branch and file already exist, and the
    //    idempotent conflict handling must not error.
    prepare_pull_request_head(
        &base,
        &engineer.token,
        &provisioned.owner,
        &provisioned.name,
        &input,
    )
    .await
    .expect("re-running pr-prep is a no-op");

    // Tear down explicitly so any panic in drop surfaces here.
    drop(server);
}

/// Polls a PR's raw REST view until `mergeable` is `true` or a short deadline
/// passes. Forgejo runs the merge-conflict check in the background after the PR
/// is created, so the field is briefly `false`/null before settling.
async fn poll_mergeable(client: &reqwest::Client, pr_url: &str, token: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Ok(resp) = client
            .get(pr_url)
            .header("Authorization", format!("token {token}"))
            .send()
            .await
        {
            if let Ok(body) = resp.json::<Value>().await {
                if body["mergeable"].as_bool() == Some(true) {
                    return true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
