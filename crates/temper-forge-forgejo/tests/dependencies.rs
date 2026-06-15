//! Offline contract tests for Forgejo issue and pull-request dependency links.
//!
//! These port the essence of the reference backends' dependency tests to the
//! mock HTTP seam: add to an existing target, idempotent duplicate add, no-op
//! remove of a missing link, `NotFound` for missing source/target, and both the
//! issue and pull-request source paths. Every request is served by a recording
//! mock client; no test touches the network.

mod support;

use support::{MockHttpClient, OWNER, REPO, block_on, body_json, forge, issue_id, pull_id};
use temper_forge_model::{ForgeError, ItemNumber};
use temper_forge_forgejo::HttpMethod;

/// Renders an issue DTO JSON body.
fn issue_json(number: u64) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "Issue {number}",
            "body": "body {number}",
            "state": "open",
            "user": {{"login": "author"}},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }}"#
    )
}

/// Renders a pull-request DTO JSON body.
fn pr_json(number: u64) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "PR {number}",
            "state": "open",
            "merged": false,
            "user": {{"login": "author"}},
            "head": {{"ref": "feature-{number}", "sha": "head{number}"}},
            "base": {{"ref": "main", "sha": "base{number}"}},
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }}"#
    )
}

/// Renders a dependency-list JSON body from a list of item numbers.
fn deps_json(numbers: &[u64]) -> String {
    let items: Vec<String> = numbers
        .iter()
        .map(|number| format!(r#"{{"number": {number}}}"#))
        .collect();
    format!("[{}]", items.join(","))
}

#[test]
fn add_issue_dependency_adds_to_existing_target() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(1)); // initial source fetch
    client.push_response(200, deps_json(&[])); // initial dependency read
    client.push_response(200, issue_json(2)); // target-exists check
    client.push_response(200, "{}"); // POST add dependency
    client.push_response(200, issue_json(1)); // refetch source
    client.push_response(200, deps_json(&[2])); // refetch dependency read
    let forge = forge(client.clone());

    let issue =
        block_on(forge.add_issue_dependency(&issue_id(1), ItemNumber::new(2))).expect("added");
    assert_eq!(issue.dependencies, vec![ItemNumber::new(2)]);

    let requests = client.recorded();
    // The dependency add posts `{ "index", "owner", "repo" }` to the issue
    // endpoint; Forgejo resolves the target by `(owner, repo, index)`.
    let post = requests
        .iter()
        .find(|request| request.method == HttpMethod::Post)
        .expect("an add request was issued");
    assert_eq!(
        post.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/1/dependencies")
    );
    let body = body_json(post);
    assert_eq!(body["index"], 2);
    assert_eq!(body["owner"], OWNER);
    assert_eq!(body["repo"], REPO);
}

#[test]
fn add_dependency_is_idempotent_when_already_present() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(1)); // source fetch
    client.push_response(200, deps_json(&[2])); // dependency already present
    let forge = forge(client.clone());

    let issue =
        block_on(forge.add_issue_dependency(&issue_id(1), ItemNumber::new(2))).expect("no-op");
    assert_eq!(issue.dependencies, vec![ItemNumber::new(2)]);
    // No target check and no POST: the source fetch and its dependency read only.
    assert_eq!(client.call_count(), 2);
    assert!(
        client
            .recorded()
            .iter()
            .all(|request| request.method == HttpMethod::Get)
    );
}

#[test]
fn remove_issue_dependency_removes_link() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(1)); // source fetch
    client.push_response(200, deps_json(&[2, 3])); // dependency read
    client.push_response(200, "{}"); // DELETE dependency
    client.push_response(200, issue_json(1)); // refetch source
    client.push_response(200, deps_json(&[3])); // refetch dependency read
    let forge = forge(client.clone());

    let issue =
        block_on(forge.remove_issue_dependency(&issue_id(1), ItemNumber::new(2))).expect("removed");
    assert_eq!(issue.dependencies, vec![ItemNumber::new(3)]);

    let delete = client
        .recorded()
        .into_iter()
        .find(|request| request.method == HttpMethod::Delete)
        .expect("a delete request was issued");
    assert_eq!(
        delete.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/1/dependencies")
    );
    assert_eq!(body_json(&delete)["index"], 2);
}

#[test]
fn remove_missing_link_is_noop() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(1)); // source fetch
    client.push_response(200, deps_json(&[3])); // dependency read; #2 absent
    let forge = forge(client.clone());

    let issue = block_on(forge.remove_issue_dependency(&issue_id(1), ItemNumber::new(2)))
        .expect("no-op remove");
    assert_eq!(issue.dependencies, vec![ItemNumber::new(3)]);
    // The source fetch and its dependency read only; no DELETE.
    assert_eq!(client.call_count(), 2);
    assert!(
        client
            .recorded()
            .iter()
            .all(|request| request.method == HttpMethod::Get)
    );
}

#[test]
fn add_dependency_missing_source_is_not_found() {
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let backend = forge(client.clone());
    let result = block_on(backend.add_issue_dependency(&issue_id(1), ItemNumber::new(2)));
    assert!(matches!(result, Err(ForgeError::NotFound(_))));
    // Only the source fetch was attempted.
    assert_eq!(client.call_count(), 1);

    // The pull-request source path behaves the same way.
    let client = MockHttpClient::new();
    client.push_response(404, r#"{"message":"not found"}"#);
    let backend = forge(client);
    let result = block_on(backend.add_pull_request_dependency(&pull_id(1), ItemNumber::new(2)));
    assert!(matches!(result, Err(ForgeError::NotFound(_))));
}

#[test]
fn add_dependency_missing_target_is_not_found() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(1)); // source fetch
    client.push_response(200, deps_json(&[])); // dependency read
    client.push_response(404, r#"{"message":"not found"}"#); // target check
    let forge = forge(client.clone());

    let result = block_on(forge.add_issue_dependency(&issue_id(1), ItemNumber::new(99)));
    assert!(matches!(result, Err(ForgeError::NotFound(_))));
    // Source fetch, dependency read, and the failed target check; no POST.
    assert_eq!(client.call_count(), 3);
}

#[test]
fn add_pull_request_dependency_uses_issue_dependency_endpoint() {
    let client = MockHttpClient::new();
    client.push_response(200, pr_json(1)); // initial source fetch
    client.push_response(200, deps_json(&[])); // initial dependency read
    client.push_response(200, issue_json(2)); // target-exists check
    client.push_response(200, "{}"); // POST add dependency
    client.push_response(200, pr_json(1)); // refetch source
    client.push_response(200, deps_json(&[2])); // refetch dependency read
    let forge = forge(client.clone());

    let pull = block_on(forge.add_pull_request_dependency(&pull_id(1), ItemNumber::new(2)))
        .expect("added");
    assert_eq!(pull.dependencies, vec![ItemNumber::new(2)]);

    let requests = client.recorded();
    // The source is fetched through the pull endpoint, but dependency links go
    // through the shared issue-number endpoint (PRs share issue numbers).
    assert_eq!(
        requests[0].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/pulls/1")
    );
    let post = requests
        .iter()
        .find(|request| request.method == HttpMethod::Post)
        .expect("an add request was issued");
    assert_eq!(
        post.path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/1/dependencies")
    );
}

#[test]
fn remove_pull_request_dependency_removes_link() {
    let client = MockHttpClient::new();
    client.push_response(200, pr_json(1)); // source fetch
    client.push_response(200, deps_json(&[2])); // dependency read
    client.push_response(200, "{}"); // DELETE dependency
    client.push_response(200, pr_json(1)); // refetch source
    client.push_response(200, deps_json(&[])); // refetch dependency read
    let forge = forge(client.clone());

    let pull = block_on(forge.remove_pull_request_dependency(&pull_id(1), ItemNumber::new(2)))
        .expect("removed");
    assert!(pull.dependencies.is_empty());
}

#[test]
fn add_dependency_unsupported_endpoint_is_invalid_request() {
    let client = MockHttpClient::new();
    client.push_response(200, issue_json(1)); // source fetch
    client.push_response(200, deps_json(&[])); // dependency read
    client.push_response(200, issue_json(2)); // target check
    client.push_response(404, r#"{"message":"Not Found"}"#); // POST: endpoint missing
    let forge = forge(client);

    // A 404 on the add endpoint is unsupported-provider, not missing-target, so
    // the backend reports InvalidRequest rather than silently claiming success.
    let result = block_on(forge.add_issue_dependency(&issue_id(1), ItemNumber::new(2)));
    assert!(matches!(result, Err(ForgeError::InvalidRequest(_))));
}

#[test]
fn read_dependencies_tolerates_unsupported_endpoint() {
    let client = MockHttpClient::new();
    client.push_response(200, pr_json(1)); // get pull request
    client.push_response(404, r#"{"message":"Not Found"}"#); // dependencies endpoint
    let forge = forge(client);

    // On a read, a 404 from the dependencies endpoint yields empty dependencies.
    let pull = block_on(forge.get_pull_request(&pull_id(1)))
        .unwrap()
        .expect("pull request present");
    assert!(pull.dependencies.is_empty());
}
