//! Offline contract tests for identity, repository, and label operations.

mod support;

use support::{block_on, body_json, forge, repo_id, MockHttpClient};
use temper_forge::{
    CreateRepository, ForgeError, RepositoryPath, RepositoryQuery, RepositorySort,
    RepositorySortField, SortDirection, UpsertLabel, UserId,
};
use temper_forge_github::HttpMethod;

fn repo_json(owner: &str, name: &str, default_branch: &str) -> String {
    format!(
        r#"{{
            "owner": {{"login": "{owner}"}},
            "name": "{name}",
            "default_branch": "{default_branch}",
            "description": "a repo",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-02T00:00:00Z"
        }}"#
    )
}

#[test]
fn current_user_hits_user_endpoint_with_github_headers() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{"login": "octocat", "name": "The Octocat", "email": "cat@example.com"}"#,
    );
    let forge = forge(client.clone());

    let user = block_on(forge.current_user()).unwrap();
    assert_eq!(user.id, UserId::new("octocat"));
    assert_eq!(user.handle, "octocat");
    assert_eq!(user.display_name.as_deref(), Some("The Octocat"));

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, "/user");
    let authorization = request
        .headers
        .iter()
        .find(|(name, _)| name == "Authorization")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert_eq!(authorization, "Bearer test-token");
    assert!(request
        .headers
        .iter()
        .any(|(name, value)| name == "Accept" && value == "application/vnd.github+json"));
    assert!(request.headers.iter().any(|(name, _)| name == "User-Agent"));
}

#[test]
fn get_user_maps_404_to_none() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"login": "alice"}"#);
    client.push_response(404, r#"{"message": "Not Found"}"#);
    let forge = forge(client.clone());

    let found = block_on(forge.get_user(&UserId::new("alice"))).unwrap();
    assert_eq!(found.unwrap().handle, "alice");
    assert_eq!(client.recorded()[0].path, "/users/alice");

    let missing = block_on(forge.get_user(&UserId::new("ghost"))).unwrap();
    assert!(missing.is_none());
}

#[test]
fn list_repositories_sorts_deterministically() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}, {}]",
            repo_json("acme", "zeta", "main"),
            repo_json("acme", "alpha", "main")
        ),
    );
    let forge = forge(client.clone());

    let repos = block_on(forge.list_repositories(RepositoryQuery::default())).unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].name, "alpha");
    assert_eq!(repos[1].name, "zeta");

    let request = client.last_request().unwrap();
    assert_eq!(request.path, "/user/repos");
    assert!(request
        .query
        .iter()
        .any(|(key, value)| key == "per_page" && value == "50"));
}

#[test]
fn list_repositories_honors_requested_sort() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}, {}]",
            repo_json("acme", "alpha", "main"),
            repo_json("acme", "zeta", "main")
        ),
    );
    let forge = forge(client);

    let repos = block_on(forge.list_repositories(RepositoryQuery {
        sort: Some(RepositorySort {
            field: RepositorySortField::Path,
            direction: SortDirection::Desc,
        }),
    }))
    .unwrap();
    assert_eq!(repos[0].name, "zeta");
    assert_eq!(repos[1].name, "alpha");
}

#[test]
fn get_repository_by_path_maps_404_to_none() {
    let client = MockHttpClient::new();
    client.push_response(200, repo_json("acme", "widgets", "main"));
    client.push_response(404, r#"{"message": "Not Found"}"#);
    let forge = forge(client.clone());

    let found =
        block_on(forge.get_repository_by_path(&RepositoryPath::new("acme", "widgets"))).unwrap();
    let found = found.unwrap();
    assert_eq!(found.id, repo_id());
    assert_eq!(found.default_branch, "main");
    assert_eq!(client.recorded()[0].path, "/repos/acme/widgets");

    let missing =
        block_on(forge.get_repository_by_path(&RepositoryPath::new("acme", "gone"))).unwrap();
    assert!(missing.is_none());
}

#[test]
fn create_repository_under_own_user() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"login": "acme"}"#); // current user
    client.push_response(201, repo_json("acme", "widgets", "main")); // create
    client.push_response(200, repo_json("acme", "widgets", "main")); // re-read
    let forge = forge(client.clone());

    let repo = block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "main".to_string(),
        description: Some("a repo".to_string()),
    }))
    .unwrap();
    assert_eq!(repo.id, repo_id());

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[1].method, HttpMethod::Post);
    assert_eq!(recorded[1].path, "/user/repos");
    let payload = body_json(&recorded[1]);
    assert_eq!(payload["name"], "widgets");
    assert_eq!(payload["description"], "a repo");
    // GitHub ignores default_branch on create; it is not sent.
    assert!(payload.get("default_branch").is_none());
}

#[test]
fn create_repository_under_organization() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"login": "bob"}"#); // current user differs from owner
    client.push_response(201, repo_json("acme", "widgets", "main"));
    client.push_response(200, repo_json("acme", "widgets", "main"));
    let forge = forge(client.clone());

    block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .unwrap();

    assert_eq!(client.recorded()[1].path, "/orgs/acme/repos");
}

#[test]
fn create_repository_patches_differing_default_branch() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"login": "acme"}"#);
    client.push_response(201, repo_json("acme", "widgets", "main"));
    client.push_response(200, repo_json("acme", "widgets", "main")); // re-read: still main
    client.push_response(200, repo_json("acme", "widgets", "develop")); // patch echo
    client.push_response(200, repo_json("acme", "widgets", "develop")); // final re-read
    let forge = forge(client.clone());

    let repo = block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "develop".to_string(),
        description: None,
    }))
    .unwrap();
    assert_eq!(repo.default_branch, "develop");

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 5);
    assert_eq!(recorded[3].method, HttpMethod::Patch);
    assert_eq!(recorded[3].path, "/repos/acme/widgets");
    assert_eq!(body_json(&recorded[3])["default_branch"], "develop");
}

#[test]
fn create_repository_maps_existing_name_to_already_exists() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"login": "acme"}"#);
    client.push_response(
        422,
        r#"{"message": "Repository creation failed.", "errors": [{"message": "name already exists on this account"}]}"#,
    );
    let forge = forge(client);

    let error = block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .unwrap_err();
    assert!(matches!(error, ForgeError::AlreadyExists(_)));
}

#[test]
fn list_labels_sorts_by_name() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"[
            {"id": 2, "name": "ready", "color": "00ff00", "description": "go"},
            {"id": 1, "name": "bug", "color": "ff0000", "description": null}
        ]"#,
    );
    let forge = forge(client.clone());

    let labels = block_on(forge.list_labels(&repo_id())).unwrap();
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0].name, "bug");
    assert_eq!(labels[0].id.as_str(), "github:acme/widgets:label:1");
    assert_eq!(labels[1].name, "ready");
    assert_eq!(labels[1].color.as_deref(), Some("00ff00"));
    assert_eq!(client.recorded()[0].path, "/repos/acme/widgets/labels");
}

#[test]
fn upsert_label_creates_missing_label() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message": "Not Found"}"#); // probe by name
    client.push_response(
        201,
        r#"{"id": 9, "name": "ready", "color": "00ff00", "description": "go"}"#,
    );
    let forge = forge(client.clone());

    let label = block_on(forge.upsert_label(
        &repo_id(),
        UpsertLabel {
            name: "ready".to_string(),
            color: Some("00ff00".to_string()),
            description: Some("go".to_string()),
        },
    ))
    .unwrap();
    assert_eq!(label.id.as_str(), "github:acme/widgets:label:9");

    let recorded = client.recorded();
    assert_eq!(recorded[0].path, "/repos/acme/widgets/labels/ready");
    assert_eq!(recorded[1].method, HttpMethod::Post);
    assert_eq!(recorded[1].path, "/repos/acme/widgets/labels");
    let payload = body_json(&recorded[1]);
    assert_eq!(payload["name"], "ready");
    assert_eq!(payload["color"], "00ff00");
}

#[test]
fn upsert_label_patches_existing_label() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{"id": 9, "name": "ready", "color": "000000", "description": null}"#,
    );
    client.push_response(
        200,
        r#"{"id": 9, "name": "ready", "color": "00ff00", "description": "go"}"#,
    );
    let forge = forge(client.clone());

    let label = block_on(forge.upsert_label(
        &repo_id(),
        UpsertLabel {
            name: "ready".to_string(),
            color: Some("00ff00".to_string()),
            description: Some("go".to_string()),
        },
    ))
    .unwrap();
    assert_eq!(label.color.as_deref(), Some("00ff00"));

    let recorded = client.recorded();
    assert_eq!(recorded[1].method, HttpMethod::Patch);
    assert_eq!(recorded[1].path, "/repos/acme/widgets/labels/ready");
}

#[test]
fn transport_error_surfaces_as_backend_error() {
    let client = MockHttpClient::new();
    client.push_transport_error("connection reset");
    let forge = forge(client);

    let error = block_on(forge.current_user()).unwrap_err();
    assert!(matches!(error, ForgeError::Backend(_)));
    assert!(error.to_string().contains("connection reset"));
}
