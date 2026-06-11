//! Phase 2b PR-prep test against a real Forgejo.
//!
//! `#[ignore]`d, so the default `cargo test` never downloads a binary, opens a
//! socket, or spawns a server. No extra environment variable is required; run it
//! with:
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_pr_prep -- --ignored
//! ```
//!
//! It starts a [`ForgejoServer`] from the declared cached Phase 2 state, then
//! proves the Phase 2b contract end to end against the real backend
//! (findings-phase-0 §1):
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

use serde_json::Value;
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_server::{prepare_pull_request_head, start_cached_provisioned_server};
use temper_testing::pull_request_input;
use temper_workflow::RoleId;

#[test]
#[ignore = "boots a real Forgejo server; run with --ignored"]
fn prep_makes_head_real_and_pr_is_mergeable() {
    temper_io_engine::block_on(async move {
    // The cached Forgejo fixture uses a *blocking* reqwest client for readiness;
    // boot it off-reactor so its nested blocking runtime lives and dies off the async
    // test thread (same pattern as the Phase 2 provisioning test).
    let cached = asupersync::runtime::spawn_blocking(start_cached_provisioned_server)
        .await
        .expect("forgejo cached provisioned state starts");
    let server = cached.server;
    let provisioned = cached.provisioned;
    let base = server.base_url().to_string();
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
        matches!(unprepared, Err(temper_forge::ForgeError::NotFound(_))),
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
    let client = temper_io_engine::http::JsonClient::new();
    let (branch_status, _) = client
        .send_expect_json(
            "GET",
            format!(
                "{base}/api/v1/repos/{}/{}/branches/{head}",
                provisioned.owner, provisioned.name
            ),
            Some(&engineer.token),
            None,
            "branch lookup",
        )
        .await;
    assert!(
        (200..300).contains(&branch_status),
        "head branch should exist after prep, got {branch_status}"
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
    })
}

/// Polls a PR's raw REST view until `mergeable` is `true` or a short deadline
/// passes. Forgejo runs the merge-conflict check in the background after the PR
/// is created, so the field is briefly `false`/null before settling.
async fn poll_mergeable(
    client: &temper_io_engine::http::JsonClient,
    pr_url: &str,
    token: &str,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Ok(resp) = client.send("GET", pr_url, Some(token), None).await {
            if let Ok(body) = serde_json::from_slice::<Value>(&resp.body) {
                if body["mergeable"].as_bool() == Some(true) {
                    return true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        temper_io_engine::runtime::sleep_for(std::time::Duration::from_millis(500)).await;
    }
}
