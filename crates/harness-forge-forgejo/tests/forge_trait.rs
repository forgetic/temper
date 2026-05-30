//! Proves [`ForgejoForge`] is a real `harness_forge::Forge` backend.
//!
//! Three checks, all offline:
//! - a read exercised end to end through a `&dyn Forge` handle against canned
//!   JSON (identity, then an issue lookup by number);
//! - a compile-time assertion that the production-typed backend implements the
//!   trait (the `_assert_*` functions below, which are type-checked but never
//!   run);
//! - a compile-only check that the backend can be handed to the workflow
//!   layer's [`Executor`], proving the generic bounds line up.

mod support;

use harness_forge::{Forge, IssueState, ItemNumber, UserId};
use harness_forge_forgejo::{ForgejoForge, ReqwestHttpClient};
use harness_workflow::{Executor, ValidatedWorkflow};
use support::{block_on, forge, repo_id, MockHttpClient, OWNER, REPO};

#[test]
fn used_through_dyn_forge_for_a_read_end_to_end() {
    let client = MockHttpClient::new();
    // 1) `current_user` → GET /user.
    client.push_response(
        200,
        r#"{"login":"octocat","full_name":"Octo Cat","email":"octo@example.com"}"#,
    );
    // 2) `get_issue_by_number` → GET the issue, then its dependency links.
    client.push_response(
        200,
        r#"{
            "number": 7,
            "title": "Wire the keystone",
            "body": "body",
            "state": "open",
            "user": {"login": "octocat"},
            "labels": [],
            "created_at": "2024-03-01T00:00:00Z",
            "updated_at": "2024-03-02T00:00:00Z"
        }"#,
    );
    client.push_response(200, "[]");

    let backend = forge(client.clone());
    // Drive the reads purely through the portable trait object.
    let forge: &dyn Forge = &backend;

    let user = block_on(forge.current_user()).unwrap();
    assert_eq!(user.handle, "octocat");
    assert_eq!(user.id, UserId::new("octocat"));

    let issue = block_on(forge.get_issue_by_number(&repo_id(), ItemNumber::new(7)))
        .unwrap()
        .expect("issue is present");
    assert_eq!(issue.number, ItemNumber::new(7));
    assert_eq!(issue.state, IssueState::Open);
    assert_eq!(issue.author_id, UserId::new("octocat"));

    // The reads went over the trait surface and hit the expected endpoints.
    let recorded = client.recorded();
    assert_eq!(recorded[0].path, "/api/v1/user");
    assert_eq!(
        recorded[1].path,
        format!("/api/v1/repos/{OWNER}/{REPO}/issues/7")
    );
}

/// Compile-time proof that the production-typed backend implements `Forge`.
///
/// Never called; it fails to type-check if the impl or its `Send + Sync` bounds
/// regress.
#[allow(dead_code)]
fn _assert_reqwest_backend_is_forge(forge: &ForgejoForge<ReqwestHttpClient>) {
    fn is_forge<T: Forge + ?Sized>(_: &T) {}
    is_forge(forge);
}

/// Compile-only proof that the backend satisfies the workflow [`Executor`]'s
/// generic bounds, in both the concrete and `&dyn Forge` forms.
///
/// Never called; constructing the executor is enough to type-check the bounds.
#[allow(dead_code)]
fn _assert_backend_drives_executor(
    workflow: &ValidatedWorkflow,
    forge: &ForgejoForge<MockHttpClient>,
) {
    let _concrete = Executor::new(workflow, forge);
    let dyn_forge: &dyn Forge = forge;
    let _erased = Executor::new(workflow, dyn_forge);
}
