//! Offline contract tests for the [`ForgeContent`] and [`ForgeAdmin`] capability
//! impls on `ForgejoForge`. Every request is served by a recording mock client;
//! no test touches the network.
//!
//! These assert the exact HTTP exchanges the absorbed `temper-forgejo-ops` REST
//! helpers performed: idempotent accept-or-conflict tolerance, base64 content
//! encoding, the token scopes/Basic-auth, the full Gitea webhook event list, and
//! the `RestError`→`ForgeError` mapping surfaced through the traits.

mod support;

use base64::Engine;
use support::{MockHttpClient, OWNER, REPO, block_on, body_json, forge, repo_id};
use temper_forge_model::{
    AccessGrant, AccessScope, CommitFile, CreateBranch, CreateRepository, ForgeAdmin, ForgeContent,
    ForgeError, NewUser, RepoPermission, RepositoryId, RepositoryPath, TokenScope, WebhookEvents,
    WebhookSpec,
};
use temper_forge_forgejo::HttpMethod;

// --- ForgeContent ------------------------------------------------------------

#[test]
fn ensure_repository_posts_org_repo_and_returns_id() {
    let client = MockHttpClient::new();
    client.push_response(201, "{}");
    let forge = forge(client.clone());

    let id = block_on(forge.ensure_repository(CreateRepository {
        owner: OWNER.to_string(),
        name: REPO.to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .unwrap();

    assert_eq!(id, repo_id());
    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, format!("/api/v1/orgs/{OWNER}/repos"));
    let body = body_json(&request);
    assert_eq!(body["name"], REPO);
    assert_eq!(body["default_branch"], "main");
    assert_eq!(body["auto_init"], true);
    assert_eq!(body["private"], false);
}

#[test]
fn ensure_repository_tolerates_existing_conflict() {
    let client = MockHttpClient::new();
    client.push_response(409, "repository already exists");
    let forge = forge(client);

    let id = block_on(forge.ensure_repository(CreateRepository {
        owner: OWNER.to_string(),
        name: REPO.to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .unwrap();
    assert_eq!(id, repo_id());
}

#[test]
fn require_repository_succeeds_when_present() {
    let client = MockHttpClient::new();
    client.push_response(200, "{}");
    let forge = forge(client.clone());

    let id = block_on(forge.require_repository(&RepositoryPath::new(OWNER, REPO))).unwrap();
    assert_eq!(id, repo_id());
    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, format!("/api/v1/repos/{OWNER}/{REPO}"));
}

#[test]
fn require_repository_missing_is_not_found() {
    let client = MockHttpClient::new();
    client.push_response(404, "not found");
    let forge = forge(client);

    let error = block_on(forge.require_repository(&RepositoryPath::new(OWNER, REPO))).unwrap_err();
    assert!(matches!(error, ForgeError::NotFound(_)));
}

#[test]
fn commit_file_base64_encodes_contents() {
    let client = MockHttpClient::new();
    client.push_response(201, "{}");
    let forge = forge(client.clone());

    block_on(forge.commit_file(
        &repo_id(),
        CommitFile {
            path: ".forgejo/workflows/ci.yml".to_string(),
            contents: b"name: ci\n".to_vec(),
            message: "add CI".to_string(),
            branch: "main".to_string(),
        },
    ))
    .unwrap();

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(
        request.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/contents/.forgejo/workflows/ci.yml")
    );
    let body = body_json(&request);
    let expected = base64::engine::general_purpose::STANDARD.encode(b"name: ci\n");
    assert_eq!(body["content"], expected);
    assert_eq!(body["message"], "add CI");
    assert_eq!(body["branch"], "main");
}

#[test]
fn commit_file_tolerates_existing_conflict() {
    let client = MockHttpClient::new();
    client.push_response(422, "the file already exists");
    let forge = forge(client);

    block_on(forge.commit_file(
        &repo_id(),
        CommitFile {
            path: "x.txt".to_string(),
            contents: b"x".to_vec(),
            message: "m".to_string(),
            branch: "main".to_string(),
        },
    ))
    .unwrap();
}

#[test]
fn create_branch_posts_branch_names() {
    let client = MockHttpClient::new();
    client.push_response(201, "{}");
    let forge = forge(client.clone());

    block_on(forge.create_branch(
        &repo_id(),
        CreateBranch {
            new_branch: "feature".to_string(),
            from_branch: "main".to_string(),
        },
    ))
    .unwrap();

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(
        request.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/branches")
    );
    let body = body_json(&request);
    assert_eq!(body["new_branch_name"], "feature");
    assert_eq!(body["old_branch_name"], "main");
}

// --- ForgeAdmin --------------------------------------------------------------

#[test]
fn ensure_owner_posts_org_and_tolerates_conflict() {
    let client = MockHttpClient::new();
    client.push_response(201, "{}");
    let backend = forge(client.clone());

    block_on(backend.ensure_owner("acme")).unwrap();
    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, "/api/v1/orgs");
    assert_eq!(body_json(&request)["username"], "acme");

    let conflict = MockHttpClient::new();
    conflict.push_response(422, "organization already exists");
    block_on(forge(conflict).ensure_owner("acme")).unwrap();
}

#[test]
fn ensure_user_posts_admin_user_with_password() {
    let client = MockHttpClient::new();
    client.push_response(201, "{}");
    let forge = forge(client.clone());

    block_on(forge.ensure_user(NewUser {
        login: "dev".to_string(),
        email: "dev@example.invalid".to_string(),
        password: "s3cret".to_string(),
    }))
    .unwrap();

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, "/api/v1/admin/users");
    let body = body_json(&request);
    assert_eq!(body["username"], "dev");
    assert_eq!(body["email"], "dev@example.invalid");
    assert_eq!(body["password"], "s3cret");
    assert_eq!(body["must_change_password"], false);
}

#[test]
fn mint_token_uses_basic_auth_unique_name_and_scopes() {
    let client = MockHttpClient::new();
    client.push_response(201, r#"{"sha1":"abc123"}"#);
    let forge = forge(client.clone());

    let token =
        block_on(forge.mint_token("dev", &[TokenScope::WriteRepository, TokenScope::ReadOrg]))
            .unwrap();
    assert_eq!(token, "abc123");

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.path, "/api/v1/users/dev/tokens");
    // Basic auth, not token auth.
    let auth = request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str())
        .unwrap();
    assert!(auth.starts_with("Basic "));

    let body = body_json(&request);
    assert_eq!(
        body["scopes"],
        serde_json::json!(["write:repository", "read:organization"])
    );
    let name = body["name"].as_str().unwrap();
    assert!(name.starts_with("temper-dev-"));
}

#[test]
fn mint_token_empty_scopes_falls_back_to_default_set() {
    let client = MockHttpClient::new();
    client.push_response(201, r#"{"sha1":"tok"}"#);
    let forge = forge(client.clone());

    block_on(forge.mint_token("dev", &[])).unwrap();
    let body = body_json(&client.last_request().unwrap());
    assert_eq!(
        body["scopes"],
        serde_json::json!([
            "write:repository",
            "write:issue",
            "write:user",
            "read:organization"
        ])
    );
}

#[test]
fn mint_token_rejects_empty_sha1() {
    let client = MockHttpClient::new();
    client.push_response(201, r#"{"sha1":""}"#);
    let forge = forge(client);

    let error = block_on(forge.mint_token("dev", &[TokenScope::WriteUser])).unwrap_err();
    assert!(matches!(error, ForgeError::Backend(_)));
}

#[test]
fn grant_access_org_owners_adds_team_member() {
    let client = MockHttpClient::new();
    // First the Owners team lookup, then the add-member PUT.
    client.push_response(200, r#"[{"name":"Owners","id":7}]"#);
    client.push_response(204, "");
    let forge = forge(client.clone());

    block_on(forge.grant_access(AccessGrant {
        login: "dev".to_string(),
        repo: repo_id(),
        scope: AccessScope::OrgOwners,
        permission: RepoPermission::Write,
    }))
    .unwrap();

    let requests = client.recorded();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, HttpMethod::Get);
    assert_eq!(requests[0].path, format!("/api/v1/orgs/{OWNER}/teams"));
    assert_eq!(requests[1].method, HttpMethod::Put);
    assert_eq!(requests[1].path, "/api/v1/teams/7/members/dev");
}

#[test]
fn grant_access_repo_collaborator_puts_permission() {
    let client = MockHttpClient::new();
    client.push_response(204, "");
    let forge = forge(client.clone());

    block_on(forge.grant_access(AccessGrant {
        login: "dev".to_string(),
        repo: repo_id(),
        scope: AccessScope::RepoCollaborator,
        permission: RepoPermission::Admin,
    }))
    .unwrap();

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Put);
    assert_eq!(
        request.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/collaborators/dev")
    );
    assert_eq!(body_json(&request)["permission"], "admin");
}

#[test]
fn grant_access_repo_collaborator_tolerates_existing() {
    let client = MockHttpClient::new();
    client.push_response(422, "user is already a collaborator");
    let forge = forge(client);

    block_on(forge.grant_access(AccessGrant {
        login: "dev".to_string(),
        repo: repo_id(),
        scope: AccessScope::RepoCollaborator,
        permission: RepoPermission::Write,
    }))
    .unwrap();
}

#[test]
fn ensure_webhook_skips_when_url_already_registered() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"[{"config":{"url":"https://hook.example/x"}}]"#);
    let forge = forge(client.clone());

    block_on(forge.ensure_webhook(
        &repo_id(),
        WebhookSpec {
            url: "https://hook.example/x".to_string(),
            secret: "shh".to_string(),
            events: WebhookEvents::All,
        },
    ))
    .unwrap();

    // Only the list call; no create.
    assert_eq!(client.call_count(), 1);
}

#[test]
fn ensure_webhook_creates_with_full_event_list() {
    let client = MockHttpClient::new();
    client.push_response(200, "[]");
    client.push_response(201, "{}");
    let forge = forge(client.clone());

    block_on(forge.ensure_webhook(
        &repo_id(),
        WebhookSpec {
            url: "https://hook.example/x".to_string(),
            secret: "shh".to_string(),
            events: WebhookEvents::All,
        },
    ))
    .unwrap();

    let requests = client.recorded();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/hooks")
    );
    let body = body_json(&requests[1]);
    assert_eq!(body["type"], "gitea");
    assert_eq!(body["active"], true);
    assert_eq!(
        body["events"],
        serde_json::json!([
            "push",
            "status",
            "issues",
            "issue_comment",
            "pull_request",
            "pull_request_sync",
            "pull_request_review_approved",
            "pull_request_review_rejected",
            "pull_request_review_comment",
            "workflow_job",
            "workflow_run",
            "action_run_failure",
            "action_run_recover",
            "action_run_success"
        ])
    );
    assert_eq!(body["config"]["url"], "https://hook.example/x");
    assert_eq!(body["config"]["content_type"], "json");
    assert_eq!(body["config"]["secret"], "shh");
}

#[test]
fn ensure_webhook_only_events_emits_named_list() {
    let client = MockHttpClient::new();
    client.push_response(200, "[]");
    client.push_response(201, "{}");
    let forge = forge(client.clone());

    block_on(forge.ensure_webhook(
        &repo_id(),
        WebhookSpec {
            url: "https://hook.example/y".to_string(),
            secret: "shh".to_string(),
            events: WebhookEvents::Only(vec!["push".to_string(), "issues".to_string()]),
        },
    ))
    .unwrap();

    let body = body_json(&client.recorded()[1]);
    assert_eq!(body["events"], serde_json::json!(["push", "issues"]));
}

#[test]
fn enable_ci_patches_has_actions() {
    let client = MockHttpClient::new();
    client.push_response(200, "{}");
    let forge = forge(client.clone());

    block_on(forge.enable_ci(&repo_id())).unwrap();

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Patch);
    assert_eq!(request.path, format!("/api/v1/repos/{OWNER}/{REPO}"));
    assert_eq!(body_json(&request)["has_actions"], true);
}

#[test]
fn enable_ci_surfaces_non_success_as_error() {
    let client = MockHttpClient::new();
    client.push_response(500, "boom");
    let forge = forge(client);

    let error = block_on(forge.enable_ci(&repo_id())).unwrap_err();
    assert!(matches!(error, ForgeError::Backend(_)));
}

#[test]
fn invalid_repository_id_is_invalid_request() {
    let client = MockHttpClient::new();
    let forge = forge(client);
    let error = block_on(forge.enable_ci(&RepositoryId::new("not-a-forgejo-id"))).unwrap_err();
    assert!(matches!(error, ForgeError::InvalidRequest(_)));
}
