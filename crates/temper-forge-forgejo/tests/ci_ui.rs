// SPDX-License-Identifier: MPL-2.0
//! The retained web-UI compatibility implementation is intentionally unreachable
//! from public CI reads while the dependent cleanup removes it.
mod support;

use serde_json::json;
use support::{MockHttpClient, block_on, forge_with_web_ui, repo_id};
use temper_forge_model::{CiJobQuery, ForgeError};

#[test]
fn configured_web_ui_credentials_do_not_enable_a_runs_fallback() {
    let client = MockHttpClient::new();
    client.push_response(404, json!({ "message": "Not Found" }).to_string());

    let result =
        block_on(forge_with_web_ui(client.clone()).list_ci_jobs(&repo_id(), CiJobQuery::default()));
    assert!(matches!(result, Err(ForgeError::Backend(_))));

    let recorded = client.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].path, "/api/v1/repos/acme/widgets/actions/runs");
    assert!(
        recorded[0]
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "token test-token")
    );
}

#[test]
fn no_matching_api_run_does_not_scrape_repository_actions_html() {
    let client = MockHttpClient::new();
    client.push_response(200, r#"{"workflow_runs":[]}"#);

    let listed =
        block_on(forge_with_web_ui(client.clone()).list_ci_jobs(&repo_id(), CiJobQuery::default()))
            .unwrap();
    assert!(listed.is_empty());
    assert_eq!(client.call_count(), 1);
    assert!(client.recorded()[0].path.starts_with("/api/v1/"));
}
