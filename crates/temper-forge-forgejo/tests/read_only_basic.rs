//! Contract tests for mutation-proof Forgejo inspection over HTTP Basic auth.
//! Every request uses the recording seam; no live Forgejo is required.

mod support;

use base64::Engine;
use support::{MockHttpClient, OWNER, REPO, block_on, repo_id};
use temper_forge_forgejo::{
    ForgejoForge, HttpClient, HttpError, HttpMethod, HttpRequest, ReadOnlyBasicAuthClient,
};
use temper_forge_model::{
    ForgeReadiness, IssueQuery, ItemListDetails, PullRequestQuery, RepositoryPath,
};

const LOGIN: &str = "site-admin";
const PASSWORD: &str = "admin-password-never-log";

fn expected_authorization() -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{LOGIN}:{PASSWORD}"));
    format!("Basic {encoded}")
}

#[test]
fn forge_reads_use_basic_auth_across_inspection_surfaces() {
    let transport = MockHttpClient::new();
    transport.push_response(
        200,
        format!(
            r#"{{
                "owner": {{"login": "{OWNER}"}},
                "name": "{REPO}",
                "default_branch": "main",
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-02-01T00:00:00Z"
            }}"#
        ),
    );
    transport.push_response(200, "[]"); // labels
    transport.push_response(200, "[]"); // webhook readiness
    transport.push_response(200, r#"{"has_actions":true}"#);
    transport.push_response(
        200,
        format!(r#"{{"login":"{LOGIN}","email":"admin@example.com"}}"#),
    );
    transport.push_response(200, "[]"); // issues
    transport.push_response(200, "[]"); // pull requests

    let forge = ForgejoForge::with_read_only_basic_client(
        "https://forge.example.com",
        LOGIN,
        PASSWORD,
        transport.clone(),
    );

    let repository = block_on(forge.get_repository_by_path(&RepositoryPath::new(OWNER, REPO)))
        .unwrap()
        .expect("repository is present");
    assert!(
        block_on(forge.list_labels(&repository.id))
            .unwrap()
            .is_empty()
    );
    assert!(
        block_on(forge.list_webhook_statuses(&repository.id))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        block_on(forge.repository_ci_enabled(&repository.id)).unwrap(),
        Some(true)
    );
    assert_eq!(
        block_on(forge.get_provisioned_user(LOGIN))
            .unwrap()
            .expect("user is present")
            .login,
        LOGIN
    );
    assert!(
        block_on(forge.list_issues(
            &repo_id(),
            IssueQuery {
                details: ItemListDetails::summary(),
                ..IssueQuery::default()
            },
        ))
        .unwrap()
        .is_empty()
    );
    assert!(
        block_on(forge.list_pull_requests(
            &repo_id(),
            PullRequestQuery {
                details: ItemListDetails::summary(),
                ..PullRequestQuery::default()
            },
        ))
        .unwrap()
        .is_empty()
    );

    let expected_paths = [
        format!("/api/v1/repos/{OWNER}/{REPO}"),
        format!("/api/v1/repos/{OWNER}/{REPO}/labels"),
        format!("/api/v1/repos/{OWNER}/{REPO}/hooks"),
        format!("/api/v1/repos/{OWNER}/{REPO}"),
        format!("/api/v1/users/{LOGIN}"),
        format!("/api/v1/repos/{OWNER}/{REPO}/issues"),
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls"),
    ];
    let requests = transport.recorded();
    assert_eq!(requests.len(), expected_paths.len());
    for (request, expected_path) in requests.iter().zip(expected_paths) {
        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.path, expected_path);
        let authorization = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str());
        assert_eq!(authorization, Some(expected_authorization().as_str()));
        assert!(
            !request
                .headers
                .iter()
                .any(|(_, value)| value.starts_with("token "))
        );
    }
}

#[test]
fn every_mutating_method_is_rejected_before_transport() {
    for method in [
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
    ] {
        let transport = MockHttpClient::new();
        let client = ReadOnlyBasicAuthClient::new(transport.clone(), LOGIN, PASSWORD);
        let error = block_on(client.execute(HttpRequest {
            method,
            path: format!("/api/v1/repos/{OWNER}/{REPO}"),
            query: Vec::new(),
            headers: Vec::new(),
            body: Some("{}".to_string()),
        }))
        .expect_err("mutation must be rejected");

        assert!(matches!(error, HttpError::ReadOnlyMethod(found) if found == method));
        assert_eq!(transport.call_count(), 0, "{method} reached transport");
    }
}

#[test]
fn credentials_and_header_material_are_redacted_from_debug_and_errors() {
    let transport = MockHttpClient::new();
    let client = ReadOnlyBasicAuthClient::new(transport.clone(), LOGIN, PASSWORD);
    let authorization = expected_authorization();
    let encoded = authorization.trim_start_matches("Basic ");

    let debug = format!("{client:?}");
    assert!(debug.contains("redacted"));
    for secret in [LOGIN, PASSWORD, authorization.as_str(), encoded] {
        assert!(!debug.contains(secret), "Debug leaked credential material");
    }

    let error = block_on(client.execute(HttpRequest {
        method: HttpMethod::Post,
        path: "/api/v1/user".to_string(),
        query: Vec::new(),
        headers: vec![("Authorization".to_string(), authorization.clone())],
        body: Some(PASSWORD.to_string()),
    }))
    .expect_err("mutation must be rejected");
    let rendered = format!("{error:?} / {error}");
    for secret in [LOGIN, PASSWORD, authorization.as_str(), encoded] {
        assert!(
            !rendered.contains(secret),
            "error leaked credential material"
        );
    }
    assert_eq!(transport.call_count(), 0);

    let forge = ForgejoForge::with_read_only_basic_client(
        "https://forge.example.com",
        LOGIN,
        PASSWORD,
        transport,
    );
    let forge_debug = format!("{forge:?}");
    for secret in [LOGIN, PASSWORD, authorization.as_str(), encoded] {
        assert!(
            !forge_debug.contains(secret),
            "backend Debug leaked credential material"
        );
    }
}
