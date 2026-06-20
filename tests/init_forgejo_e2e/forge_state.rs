use temper_engine_io::http::BlockingJsonClient;
use temper_provision::BOT_USER;

use super::{REPO_NAME, REPO_OWNER};

pub(super) fn assert_forge_state(
    rest: &BlockingJsonClient,
    base_url: &str,
    admin_token: &str,
    webhook_url: &str,
) {
    let token = Some(admin_token);

    let (status, _) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/orgs/{REPO_OWNER}"),
        token,
        None,
        "get org",
    );
    assert_eq!(status, 200, "org {REPO_OWNER} must exist");

    let (status, repo) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}"),
        token,
        None,
        "get repo",
    );
    assert_eq!(status, 200, "repo {REPO_OWNER}/{REPO_NAME} must exist");
    assert_eq!(
        repo["has_actions"].as_bool(),
        Some(true),
        "Actions/CI must be enabled on the repo"
    );

    for path in [".forgejo/workflows/ci.yml", ".temper-ci/main.txt"] {
        let (status, _) = rest.send_expect_json(
            "GET",
            format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/contents/{path}"),
            token,
            None,
            "get repository seed file",
        );
        assert_eq!(
            status, 404,
            "temper init must not commit project seed file {path}"
        );
    }

    let labels = repo_label_names(rest, base_url, admin_token);
    for label in temper_reference_delivery::basic_delivery_workflow().labels() {
        assert!(
            labels.iter().any(|name| name == label.as_str()),
            "workflow label `{}` must exist on the repo (have: {labels:?})",
            label.as_str()
        );
    }

    for login in ["architect", "engineer", BOT_USER] {
        let (status, _) = rest.send_expect_json(
            "GET",
            format!("{base_url}/api/v1/users/{login}"),
            token,
            None,
            "get user",
        );
        assert_eq!(status, 200, "user `{login}` must be provisioned");
    }

    let hooks = repo_webhook_urls(rest, base_url, admin_token);
    assert!(
        hooks.iter().any(|url| url == webhook_url),
        "a webhook pointing at {webhook_url} must be registered (have: {hooks:?})"
    );
}

pub(super) fn forge_object_counts(
    rest: &BlockingJsonClient,
    base_url: &str,
    admin_token: &str,
) -> (usize, usize) {
    let labels = repo_label_names(rest, base_url, admin_token).len();
    let hooks = repo_webhook_urls(rest, base_url, admin_token).len();
    (labels, hooks)
}

fn repo_label_names(rest: &BlockingJsonClient, base_url: &str, admin_token: &str) -> Vec<String> {
    let (status, labels) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/labels?limit=100"),
        Some(admin_token),
        None,
        "list labels",
    );
    assert_eq!(status, 200, "listing labels must succeed");
    labels
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|l| l["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn repo_webhook_urls(rest: &BlockingJsonClient, base_url: &str, admin_token: &str) -> Vec<String> {
    let (status, hooks) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/hooks"),
        Some(admin_token),
        None,
        "list webhooks",
    );
    assert_eq!(status, 200, "listing webhooks must succeed");
    hooks
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|h| h["config"]["url"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
