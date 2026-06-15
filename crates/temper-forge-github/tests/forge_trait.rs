//! Proves `GitHubForge` is usable through the portable `Forge` trait object.

mod support;

use std::sync::Arc;
use support::{MockHttpClient, block_on, forge, issue_id, repo_id};
use temper_forge_model::{Forge, ForgeError, ItemNumber, UserId};

fn as_dyn(forge: temper_forge_github::GitHubForge<MockHttpClient>) -> Arc<dyn Forge> {
    Arc::new(forge)
}

#[test]
fn backend_is_usable_as_a_trait_object() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"login": "octocat"}"#);
    let backend: Arc<dyn Forge> = as_dyn(forge(client));

    let user = block_on(backend.current_user()).unwrap();
    assert_eq!(user.id, UserId::new("octocat"));
}

#[test]
fn trait_object_reads_issues() {
    let client = MockHttpClient::new();
    client.push_response(
        200,
        r#"{
            "number": 7,
            "title": "via trait",
            "state": "open",
            "user": {"login": "author"},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }"#,
    );
    let backend: Arc<dyn Forge> = as_dyn(forge(client));

    let issue = block_on(backend.get_issue_by_number(&repo_id(), ItemNumber::new(7)))
        .unwrap()
        .unwrap();
    assert_eq!(issue.id, issue_id(7));
    assert_eq!(issue.title, "via trait");
}

#[test]
fn trait_object_surfaces_unsupported_dependency_links() {
    let client = MockHttpClient::new();
    let backend: Arc<dyn Forge> = as_dyn(forge(client));

    let error =
        block_on(backend.add_issue_dependency(&issue_id(7), ItemNumber::new(9))).unwrap_err();
    assert!(matches!(error, ForgeError::InvalidRequest(_)));
}
