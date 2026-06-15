//! Behavior tests for issue #70: provisioning onto an existing, content-bearing
//! repo with repo-scoped access.
//!
//! `#[ignore]`d like the other real-backend tests: the default `cargo test`
//! never downloads a Forgejo binary or opens a socket. Run with:
//!
//! ```sh
//! cargo test -p temper-provision-forgejo-cli --test existing_repo_access -- --ignored
//! ```
//!
//! Each test boots a throwaway [`ForgejoServer`], bootstraps an admin token, and
//! drives the demo operator [`provision_world`] (not the testing-crate copy) so
//! the `--existing-repo` and `--access repo-collaborator` paths are exercised
//! end to end against a real Forgejo:
//!
//! - `existing_repo` leaves a pre-seeded repo's `.forgejo/workflows/ci.yml` and
//!   history untouched (no marker CI commit, no sentinel), still upserts labels
//!   and enables Actions, and registers the webhook;
//! - `--access repo-collaborator` grants role users + the `bot` repo-scoped
//!   `write` and never adds them to the org Owners team;
//! - `--existing-repo` against a missing repo errors clearly.

use serde_json::Value;
use temper_provision_forgejo_cli::provision::{
    AccessScope, BOT_USER, ProvisionOptions, provision_and_seed, provision_world,
};
use temper_testing::forgejo_server::ForgejoServer;
use temper_testing::forgejo_server::provision::bootstrap_admin;
use temper_testing::{runner_config, workflow};

const OWNER: &str = "ai";
const SENTINEL_PATH: &str = ".forgejo/workflows/ci.yml";
const SENTINEL_BODY: &str = "name: project-owned-ci\non: [push]\njobs:\n  noop:\n    runs-on: host\n    steps:\n      - run: echo project owns its CI\n";

/// Boots a throwaway server on a blocking thread (its readiness poll uses a
/// blocking client that must not be dropped on the reactor) and returns it with
/// a freshly bootstrapped admin token.
async fn start_server_with_admin() -> (ForgejoServer, String) {
    skein::runtime::spawn_blocking(|| {
        let server = ForgejoServer::start().expect("forgejo server boots");
        let admin_token = bootstrap_admin(&server).expect("admin bootstrap mints a token");
        (server, admin_token)
    })
    .await
}

fn http() -> temper_engine_io::http::JsonClient {
    temper_engine_io::http::JsonClient::new()
}

async fn create_org(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
) {
    let (status, _) = client
        .send_expect_json(
            "POST",
            format!("{base}/api/v1/orgs"),
            Some(admin),
            Some(&serde_json::json!({ "username": owner })),
            "create org",
        )
        .await;
    assert!((200..300).contains(&status), "create org failed: {status}");
}

async fn create_repo_with_sentinel(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    name: &str,
) {
    let (status, _) = client
        .send_expect_json(
            "POST",
            format!("{base}/api/v1/orgs/{owner}/repos"),
            Some(admin),
            Some(&serde_json::json!({
                "name": name,
                "default_branch": "main",
                "auto_init": true,
                "private": false,
            })),
            "create repo",
        )
        .await;
    assert!((200..300).contains(&status), "create repo failed: {status}");

    // Commit the project's own CI file so we can prove `--existing-repo` leaves
    // it byte-for-byte untouched.
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(SENTINEL_BODY);
    let (status, _) = client
        .send_expect_json(
            "POST",
            format!("{base}/api/v1/repos/{owner}/{name}/contents/{SENTINEL_PATH}"),
            Some(admin),
            Some(&serde_json::json!({
                "content": encoded,
                "message": "project: add own CI",
                "branch": "main",
            })),
            "commit sentinel",
        )
        .await;
    assert!(
        (200..300).contains(&status),
        "commit sentinel failed: {status}"
    );
}

/// Reads a file's decoded contents from the repo, or `None` if absent.
async fn file_contents(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    name: &str,
    path: &str,
) -> Option<String> {
    let (status, body) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}/contents/{path}"),
            Some(admin),
            None,
            "contents request",
        )
        .await;
    if !(200..300).contains(&status) {
        return None;
    }
    let encoded = body["content"].as_str().expect("content field is a string");
    // Forgejo returns base64 with embedded newlines.
    use base64::Engine;
    let cleaned: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .expect("content decodes");
    Some(String::from_utf8(bytes).expect("content is utf-8"))
}

/// The number of commits on `main`. Used to prove `--existing-repo` adds none.
async fn commit_count(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    name: &str,
) -> usize {
    let (status, body) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}/commits?sha=main&limit=50"),
            Some(admin),
            None,
            "list commits",
        )
        .await;
    assert!(
        (200..300).contains(&status),
        "list commits failed: {status}"
    );
    body.as_array().expect("commits is an array").len()
}

/// The collaborator permission Forgejo reports for `login` on the repo, or
/// `None` if `login` is not a collaborator.
async fn collaborator_permission(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    name: &str,
    login: &str,
) -> Option<String> {
    // 404 when not a collaborator; otherwise `{ "permission": "...", ... }`.
    let (status, body) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}/collaborators/{login}/permission"),
            Some(admin),
            None,
            "collaborator permission",
        )
        .await;
    if status == 404 {
        return None;
    }
    assert!(
        (200..300).contains(&status),
        "collaborator permission failed for {login}: {status}"
    );
    body["permission"].as_str().map(str::to_string)
}

/// Whether `login` is a member of the org Owners team.
async fn is_owners_member(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    login: &str,
) -> bool {
    let (_, teams) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/orgs/{owner}/teams"),
            Some(admin),
            None,
            "list teams",
        )
        .await;
    let owners_id = teams
        .as_array()
        .and_then(|teams| {
            teams
                .iter()
                .find(|team| team["name"].as_str() == Some("Owners"))
        })
        .and_then(|team| team["id"].as_i64())
        .expect("org has an Owners team");
    let (status, _) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/teams/{owners_id}/members/{login}"),
            Some(admin),
            None,
            "team membership",
        )
        .await;
    (200..300).contains(&status)
}

async fn label_names(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    name: &str,
) -> Vec<String> {
    let (_, body) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}/labels?limit=100"),
            Some(admin),
            None,
            "list labels",
        )
        .await;
    body.as_array()
        .expect("labels is an array")
        .iter()
        .filter_map(|label| label["name"].as_str().map(str::to_string))
        .collect()
}

async fn has_actions(
    client: &temper_engine_io::http::JsonClient,
    base: &str,
    admin: &str,
    owner: &str,
    name: &str,
) -> bool {
    let (_, body) = client
        .send_expect_json(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}"),
            Some(admin),
            None,
            "repo request",
        )
        .await;
    body["has_actions"].as_bool().unwrap_or(false)
}

#[test]
#[ignore = "boots a real Forgejo server; run with --ignored"]
fn existing_repo_repo_collaborator_leaves_content_and_grants_repo_scope() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let (server, admin) = start_server_with_admin().await;
        let base = server.base_url().to_string();
        let name = "smith";
        let client = http();

        // A real, content-bearing repo on a shared org (the `ai` org also hosts
        // `temper`), with the project's own CI already committed.
        create_org(&client, &base, &admin, OWNER).await;
        create_repo_with_sentinel(&client, &base, &admin, OWNER, name).await;
        let commits_before = commit_count(&client, &base, &admin, OWNER, name).await;

        let config = runner_config();
        let workflow = workflow();
        let provisioned = provision_world(
            &cx,
            &base,
            &admin,
            OWNER,
            name,
            &config.role_bindings,
            "main",
            &workflow,
            ProvisionOptions {
                existing_repo: true,
                access: AccessScope::RepoCollaborator,
            },
        )
        .await
        .expect("provision against existing repo succeeds");

        // The repo's own CI file is byte-for-byte untouched (no marker CI commit).
        let after = file_contents(&client, &base, &admin, OWNER, name, SENTINEL_PATH)
            .await
            .expect("project CI file still present");
        assert_eq!(
            after, SENTINEL_BODY,
            "project-owned CI must not be overwritten"
        );
        assert!(
            !after.contains("[ci-pass]"),
            "marker CI workflow must not have been committed over the project's CI"
        );

        // No provisioning commits were added to main (no marker CI, no sentinel).
        let commits_after = commit_count(&client, &base, &admin, OWNER, name).await;
        assert_eq!(
            commits_after, commits_before,
            "--existing-repo must add no commits to main"
        );

        // Labels were still upserted and Actions enabled.
        let labels = label_names(&client, &base, &admin, OWNER, name).await;
        let declared: Vec<String> = workflow
            .compile()
            .labels()
            .labels()
            .iter()
            .map(|spec| spec.id.to_string())
            .collect();
        for label in &declared {
            assert!(labels.contains(label), "label {label} should be upserted");
        }
        assert!(
            has_actions(&client, &base, &admin, OWNER, name).await,
            "Actions must be enabled"
        );

        // Every role user and the bot is a repo collaborator with `write`, and none
        // were added to the Owners team.
        for identity in provisioned.roles.values() {
            assert_eq!(
                collaborator_permission(&client, &base, &admin, OWNER, name, &identity.user).await,
                Some("write".to_string()),
                "role {} should have repo-scoped write",
                identity.user
            );
            assert!(
                !is_owners_member(&client, &base, &admin, OWNER, &identity.user).await,
                "role {} must not be an Owners-team member under repo-collaborator access",
                identity.user
            );
        }
        assert_eq!(
            collaborator_permission(&client, &base, &admin, OWNER, name, BOT_USER).await,
            Some("write".to_string()),
            "bot should have repo-scoped write so it can merge approved PRs"
        );
        assert!(
            !is_owners_member(&client, &base, &admin, OWNER, BOT_USER).await,
            "bot must not be an Owners-team member under repo-collaborator access"
        );

        drop(server);
    })
}

#[test]
#[ignore = "boots a real Forgejo server; run with --ignored"]
fn existing_repo_errors_when_repo_absent() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let (server, admin) = start_server_with_admin().await;
        let base = server.base_url().to_string();
        let client = http();

        // The org exists, but the named repo does not.
        create_org(&client, &base, &admin, OWNER).await;

        let config = runner_config();
        let workflow = workflow();
        let error = provision_world(
            &cx,
            &base,
            &admin,
            OWNER,
            "does-not-exist",
            &config.role_bindings,
            "main",
            &workflow,
            ProvisionOptions {
                existing_repo: true,
                access: AccessScope::RepoCollaborator,
            },
        )
        .await
        .expect_err("--existing-repo against an absent repo must error");
        let message = error.to_string();
        assert!(
            message.contains("does-not-exist") && message.contains("does not exist"),
            "error should name the missing repo: {message}"
        );

        drop(server);
    })
}

#[test]
#[ignore = "boots a real Forgejo server; run with --ignored"]
fn existing_repo_with_webhook_registers_hook_without_touching_ci() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        // Exercises the full `provision_and_seed` path with `--existing-repo` and a
        // webhook, mirroring the intended Smith caller (`--seed-intake no`).
        let (server, admin) = start_server_with_admin().await;
        let base = server.base_url().to_string();
        let name = "smith-webhook";
        let client = http();

        create_org(&client, &base, &admin, OWNER).await;
        create_repo_with_sentinel(&client, &base, &admin, OWNER, name).await;

        // A webhook secret file on disk.
        let dir = std::env::temp_dir().join(format!("temper-issue70-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let secret_file = dir.join("webhook-secret");
        std::fs::write(&secret_file, "s3cr3t").expect("write secret");

        let workflow = workflow();
        let (_provisioned, issue) = provision_and_seed(
            &cx,
            &base,
            &admin,
            OWNER,
            name,
            Some("http://127.0.0.1:9999/forgejo/webhook"),
            Some(secret_file.as_path()),
            None, // --seed-intake no
            &workflow,
            ProvisionOptions {
                existing_repo: true,
                access: AccessScope::RepoCollaborator,
            },
        )
        .await
        .expect("provision_and_seed against existing repo succeeds");
        assert!(issue.is_none(), "no intake issue seeded with seed=None");

        // The webhook is registered.
        let (_, hooks) = client
            .send_expect_json(
                "GET",
                format!("{base}/api/v1/repos/{OWNER}/{name}/hooks"),
                Some(&admin),
                None,
                "list hooks",
            )
            .await;
        let registered = hooks
            .as_array()
            .expect("hooks is an array")
            .iter()
            .any(|hook| {
                hook.pointer("/config/url").and_then(Value::as_str)
                    == Some("http://127.0.0.1:9999/forgejo/webhook")
            });
        assert!(registered, "webhook should be registered");

        // The project's CI is still untouched.
        let after = file_contents(&client, &base, &admin, OWNER, name, SENTINEL_PATH)
            .await
            .expect("project CI file still present");
        assert_eq!(after, SENTINEL_BODY, "project-owned CI must survive");

        std::fs::remove_dir_all(&dir).ok();
        drop(server);
    })
}
