//! Offline contract tests for Forgejo identity, repository, and label
//! operations. Every request is served by a recording mock client; no test
//! touches the network.

mod support;

use support::{MockHttpClient, OWNER, REPO, block_on, body_json, forge, repo_id};
use temper_forge_model::{
    CreateRepository, RepositoryPath, RepositoryQuery, RepositorySort, RepositorySortField,
    SortDirection, UpsertLabel, UserId,
};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge, HttpMethod};

/// Renders a user DTO JSON body.
fn user_json(login: &str, full_name: &str, email: &str) -> String {
    format!(r#"{{"login":"{login}","full_name":"{full_name}","email":"{email}","extra":true}}"#)
}

/// Renders a repository DTO JSON body.
fn repo_json(owner: &str, name: &str, created: &str, updated: &str) -> String {
    format!(
        r#"{{
            "owner": {{"login": "{owner}"}},
            "name": "{name}",
            "full_name": "{owner}/{name}",
            "default_branch": "main",
            "description": "the {name} repo",
            "created_at": "{created}",
            "updated_at": "{updated}"
        }}"#
    )
}

// --- identity ---------------------------------------------------------------

#[test]
fn current_user_maps_fields() {
    let client = MockHttpClient::new();
    client.push_response(200, user_json("octocat", "The Octocat", "cat@example.com"));
    let forge = forge(client.clone());

    let user = block_on(forge.current_user()).unwrap();
    assert_eq!(user.id, UserId::new("octocat"));
    assert_eq!(user.handle, "octocat");
    assert_eq!(user.display_name.as_deref(), Some("The Octocat"));
    assert_eq!(user.email.as_deref(), Some("cat@example.com"));

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, "/api/v1/user");
}

#[test]
fn current_user_blank_optionals_map_to_none() {
    let client = MockHttpClient::new();
    client.push_response(200, user_json("ghost", "", ""));
    let forge = forge(client);

    let user = block_on(forge.current_user()).unwrap();
    assert_eq!(user.handle, "ghost");
    assert_eq!(user.display_name, None);
    assert_eq!(user.email, None);
}

#[test]
fn get_user_maps_present_and_absent() {
    let client = MockHttpClient::new();
    client.push_response(200, user_json("carol", "Carol", "carol@example.com"));
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client.clone());

    let present = block_on(forge.get_user(&UserId::new("carol")))
        .unwrap()
        .expect("user present");
    assert_eq!(present.id, UserId::new("carol"));
    assert_eq!(present.display_name.as_deref(), Some("Carol"));
    assert_eq!(client.recorded()[0].path, "/api/v1/users/carol".to_string());

    assert!(
        block_on(forge.get_user(&UserId::new("nobody")))
            .unwrap()
            .is_none()
    );
}

// --- repositories -----------------------------------------------------------

#[test]
fn get_repository_by_path_maps_fields() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        repo_json(OWNER, REPO, "2024-01-01T00:00:00Z", "2024-02-02T00:00:00Z"),
    );
    let forge = forge(client.clone());

    let repo = block_on(forge.get_repository_by_path(&RepositoryPath::new(OWNER, REPO)))
        .unwrap()
        .expect("repository present");
    assert_eq!(repo.id, repo_id());
    assert_eq!(repo.owner, OWNER);
    assert_eq!(repo.name, REPO);
    assert_eq!(repo.default_branch, "main");
    assert_eq!(repo.description.as_deref(), Some("the widgets repo"));

    let request = client.last_request().unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(request.path, format!("/api/v1/repos/{OWNER}/{REPO}"));
}

#[test]
fn get_repository_by_path_404_is_none() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let forge = forge(client);

    assert!(
        block_on(forge.get_repository_by_path(&RepositoryPath::new(OWNER, "missing")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn get_repository_by_id_delegates_to_path_lookup() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        repo_json(OWNER, REPO, "2024-01-01T00:00:00Z", "2024-02-02T00:00:00Z"),
    );
    let forge = forge(client.clone());

    let repo = block_on(forge.get_repository(&repo_id()))
        .unwrap()
        .expect("repository present");
    assert_eq!(repo.id, repo_id());
    assert_eq!(
        client.last_request().unwrap().path,
        format!("/api/v1/repos/{OWNER}/{REPO}")
    );
}

/// Builds a list body of three repositories with distinct path/created/updated
/// orderings so each sort field yields a unique result.
fn three_repo_body() -> String {
    format!(
        "[{},{},{}]",
        repo_json(
            "acme",
            "alpha",
            "2024-03-01T00:00:00Z",
            "2024-01-05T00:00:00Z"
        ),
        repo_json(
            "acme",
            "beta",
            "2024-01-01T00:00:00Z",
            "2024-03-05T00:00:00Z"
        ),
        repo_json(
            "zen",
            "core",
            "2024-02-01T00:00:00Z",
            "2024-02-05T00:00:00Z"
        ),
    )
}

fn names(repos: &[temper_forge_model::Repository]) -> Vec<String> {
    repos.iter().map(|repo| repo.name.clone()).collect()
}

#[test]
fn list_repositories_sorts_by_path_created_and_updated() {
    let sort = |field| {
        Some(RepositorySort {
            field,
            direction: SortDirection::Asc,
        })
    };

    for (field, expected) in [
        (RepositorySortField::Path, vec!["alpha", "beta", "core"]),
        (
            RepositorySortField::CreatedAt,
            vec!["beta", "core", "alpha"],
        ),
        (
            RepositorySortField::UpdatedAt,
            vec!["alpha", "core", "beta"],
        ),
    ] {
        let client = MockHttpClient::new();
        client.push_response(200, three_repo_body());
        let forge = forge(client.clone());

        let repos =
            block_on(forge.list_repositories(RepositoryQuery { sort: sort(field) })).unwrap();
        assert_eq!(names(&repos), expected, "field {field:?}");
        assert_eq!(client.recorded()[0].path, "/api/v1/user/repos");
    }
}

#[test]
fn list_repositories_descending_reverses_path_order() {
    let client = MockHttpClient::new();
    client.push_response(200, three_repo_body());
    let forge = forge(client);

    let query = RepositoryQuery {
        sort: Some(RepositorySort {
            field: RepositorySortField::Path,
            direction: SortDirection::Desc,
        }),
    };
    let repos = block_on(forge.list_repositories(query)).unwrap();
    assert_eq!(names(&repos), vec!["core", "beta", "alpha"]);
}

#[test]
fn list_repositories_paginates_until_short_page() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        format!(
            "[{}]",
            repo_json(
                "acme",
                "alpha",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z"
            )
        ),
    );
    client.push_response(
        200,
        format!(
            "[{}]",
            repo_json(
                "acme",
                "beta",
                "2024-02-01T00:00:00Z",
                "2024-02-01T00:00:00Z"
            )
        ),
    );
    client.push_response(200, "[]");
    let config = ForgejoConfig::new("https://forge.example.com", "test-token").with_page_limit(1);
    let forge = ForgejoForge::with_client(config, client.clone());

    let repos = block_on(forge.list_repositories(RepositoryQuery::default())).unwrap();
    assert_eq!(names(&repos), vec!["alpha", "beta"]);
    assert_eq!(client.call_count(), 3);
    let pages: Vec<String> = client
        .recorded()
        .iter()
        .filter_map(|request| {
            request
                .query
                .iter()
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.clone())
        })
        .collect();
    assert_eq!(pages, vec!["1", "2", "3"]);
}

#[test]
fn create_repository_under_current_user_posts_to_user_repos() {
    let client = MockHttpClient::new();
    // current_user, then the create POST (empty success body), then the
    // normalizing re-fetch by path.
    client.push_response(200, user_json("acme", "Acme Bot", "bot@example.com"));
    client.push_response(201, "");
    client.push_response(
        200,
        repo_json(
            "acme",
            "widgets",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        ),
    );
    let forge = forge(client.clone());

    let input = CreateRepository {
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "main".to_string(),
        description: Some("the widgets repo".to_string()),
    };
    let repo = block_on(forge.create_repository(input)).unwrap();

    // The returned record is normalized through the re-fetch mapping.
    assert_eq!(repo.id, repo_id());
    assert_eq!(repo.default_branch, "main");
    assert_eq!(repo.description.as_deref(), Some("the widgets repo"));

    let requests = client.recorded();
    assert_eq!(requests[0].path, "/api/v1/user");
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(requests[1].path, "/api/v1/user/repos");
    let body = body_json(&requests[1]);
    assert_eq!(body["name"], "widgets");
    assert_eq!(body["default_branch"], "main");
    assert_eq!(body["description"], "the widgets repo");
    assert_eq!(requests[2].path, format!("/api/v1/repos/{OWNER}/{REPO}"));
}

#[test]
fn create_repository_for_other_owner_posts_to_org_repos() {
    let client = MockHttpClient::new();
    client.push_response(200, user_json("acme", "Acme Bot", ""));
    client.push_response(201, "");
    client.push_response(
        200,
        repo_json(
            "research",
            "engine",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        ),
    );
    let forge = forge(client.clone());

    let input = CreateRepository {
        owner: "research".to_string(),
        name: "engine".to_string(),
        default_branch: "trunk".to_string(),
        description: None,
    };
    let repo = block_on(forge.create_repository(input)).unwrap();
    assert_eq!(repo.owner, "research");
    assert_eq!(repo.name, "engine");

    let requests = client.recorded();
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(requests[1].path, "/api/v1/org/research/repos");
    let body = body_json(&requests[1]);
    assert_eq!(body["name"], "engine");
    assert_eq!(body["default_branch"], "trunk");
    // No description was provided, so the field is omitted.
    assert!(body.get("description").is_none());
    assert_eq!(requests[2].path, "/api/v1/repos/research/engine");
}

#[test]
fn create_repository_conflict_maps_to_already_exists() {
    let client = MockHttpClient::new();
    client.push_response(200, user_json("acme", "Acme Bot", ""));
    client.push_response(409, r#"{"message":"repo already exists"}"#);
    let forge = forge(client);

    let input = CreateRepository {
        owner: "acme".to_string(),
        name: "widgets".to_string(),
        default_branch: "main".to_string(),
        description: None,
    };
    let error = block_on(forge.create_repository(input)).unwrap_err();
    assert!(matches!(error, temper_forge_model::ForgeError::AlreadyExists(_)));
}

// --- labels -----------------------------------------------------------------

/// Renders a label DTO JSON body.
fn label_json(id: u64, name: &str, color: &str, description: &str) -> String {
    format!(r#"{{"id":{id},"name":"{name}","color":"{color}","description":"{description}"}}"#)
}

#[test]
fn list_labels_maps_and_sorts_by_name() {
    let client = MockHttpClient::new();
    let body = format!(
        "[{},{}]",
        label_json(2, "ready", "00ff00", "ready to start"),
        label_json(1, "blocked", "ff0000", ""),
    );
    client.push_response(200, body);
    let forge = forge(client.clone());

    let labels = block_on(forge.list_labels(&repo_id())).unwrap();
    let names: Vec<&str> = labels.iter().map(|label| label.name.as_str()).collect();
    assert_eq!(names, vec!["blocked", "ready"]);

    let blocked = &labels[0];
    assert_eq!(blocked.repo_id, repo_id());
    assert_eq!(
        blocked.id,
        temper_forge_model::LabelId::new(format!("forgejo:{OWNER}/{REPO}:label:1"))
    );
    assert_eq!(blocked.color.as_deref(), Some("ff0000"));
    // An empty description maps to None.
    assert_eq!(blocked.description, None);

    let ready = &labels[1];
    assert_eq!(ready.color.as_deref(), Some("00ff00"));
    assert_eq!(ready.description.as_deref(), Some("ready to start"));

    assert_eq!(
        client.recorded()[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );
}

#[test]
fn upsert_label_creates_when_absent() {
    let client = MockHttpClient::new();
    // List by name finds no match, then the create POST returns the new label.
    client.push_response(200, format!("[{}]", label_json(1, "other", "cccccc", "")));
    client.push_response(201, label_json(7, "ready", "00ff00", "ready to start"));
    let forge = forge(client.clone());

    let input = UpsertLabel {
        name: "ready".to_string(),
        color: Some("00ff00".to_string()),
        description: Some("ready to start".to_string()),
    };
    let label = block_on(forge.upsert_label(&repo_id(), input)).unwrap();
    assert_eq!(label.name, "ready");
    assert_eq!(
        label.id,
        temper_forge_model::LabelId::new(format!("forgejo:{OWNER}/{REPO}:label:7"))
    );
    assert_eq!(label.color.as_deref(), Some("00ff00"));

    let requests = client.recorded();
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels")
    );
    let body = body_json(&requests[1]);
    assert_eq!(body["name"], "ready");
    assert_eq!(body["color"], "00ff00");
    assert_eq!(body["description"], "ready to start");
}

#[test]
fn upsert_label_patches_when_present() {
    let client = MockHttpClient::new();
    // List by name finds the existing label id, then the PATCH returns it.
    client.push_response(
        200,
        format!("[{}]", label_json(5, "ready", "00ff00", "old")),
    );
    client.push_response(200, label_json(5, "ready", "112233", "updated"));
    let forge = forge(client.clone());

    let input = UpsertLabel {
        name: "ready".to_string(),
        color: Some("112233".to_string()),
        description: Some("updated".to_string()),
    };
    let label = block_on(forge.upsert_label(&repo_id(), input)).unwrap();
    assert_eq!(label.color.as_deref(), Some("112233"));
    assert_eq!(label.description.as_deref(), Some("updated"));

    let requests = client.recorded();
    assert_eq!(requests[1].method, HttpMethod::Patch);
    assert_eq!(
        requests[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/labels/5")
    );
    let body = body_json(&requests[1]);
    assert_eq!(body["name"], "ready");
    assert_eq!(body["color"], "112233");
    assert_eq!(body["description"], "updated");
}
