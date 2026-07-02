// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use temper_config::{ExposeSecret, RepoPath, Resolved, WorkerPoolSettings};

use super::super::finding::{CheckCategory, CheckFinding};
use super::http::BlockingHttpClient;
use super::{push_network_url_error, validate_http_base_url};

pub(super) fn add_engine_forge_checks(resolved: &Resolved, findings: &mut Vec<CheckFinding>) {
    let Some(base_url) = forge_base_url(resolved, "engine", findings) else {
        return;
    };
    let Some(token) = resolved.forge.admin_token.as_ref() else {
        findings.push(CheckFinding::online_error(
            "engine",
            CheckCategory::Auth,
            "Forgejo authentication cannot run: engine/admin forge token is unavailable",
        ));
        return;
    };

    let Some(client) = blocking_http_client("engine", findings) else {
        return;
    };
    let credential = "engine Forge credential";
    if !probe_forge_auth(
        &client,
        base_url,
        token.expose_secret(),
        "engine",
        credential,
        findings,
    ) {
        return;
    }

    for repo in &resolved.engine.repos {
        probe_repo_visible(
            &client,
            base_url,
            token.expose_secret(),
            "engine",
            credential,
            repo,
            findings,
        );
    }
}

pub(super) fn add_worker_forge_visibility_checks(
    resolved: &Resolved,
    pool_name: Option<&str>,
    findings: &mut Vec<CheckFinding>,
) {
    let Some(base_url) = resolved.forge.url.as_deref() else {
        // A worker may use an explicit git_base_url with no Forgejo API base URL;
        // the offline worker check already reports when neither URL exists.
        return;
    };
    if let Err(message) = validate_http_base_url(base_url, "Forgejo base URL") {
        push_network_url_error("worker", message, findings);
        return;
    }

    let Some(client) = blocking_http_client("worker", findings) else {
        return;
    };
    match pool_name {
        Some(name) => {
            if let Some(pool) = resolved.worker.pools.iter().find(|pool| pool.name == name) {
                add_pool_forge_checks(resolved, &client, base_url, pool, findings);
            }
        }
        None if !resolved.worker.pools.is_empty() => {
            for pool in &resolved.worker.pools {
                add_pool_forge_checks(resolved, &client, base_url, pool, findings);
            }
        }
        None => add_capability_forge_checks(resolved, &client, base_url, findings),
    }
}

fn blocking_http_client(
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) -> Option<BlockingHttpClient> {
    match BlockingHttpClient::new() {
        Ok(client) => Some(client),
        Err(error) => {
            findings.push(CheckFinding::online_error(
                scope,
                CheckCategory::Network,
                format!("failed to initialize HTTP client for online Forgejo checks: {error}"),
            ));
            None
        }
    }
}

fn add_pool_forge_checks(
    resolved: &Resolved,
    client: &BlockingHttpClient,
    base_url: &str,
    pool: &WorkerPoolSettings,
    findings: &mut Vec<CheckFinding>,
) {
    for role in &pool.roles {
        add_role_repo_forge_checks(
            resolved,
            client,
            base_url,
            role,
            &pool.repos,
            &format!("worker pool `{}`", pool.name),
            findings,
        );
    }
}

fn add_capability_forge_checks(
    resolved: &Resolved,
    client: &BlockingHttpClient,
    base_url: &str,
    findings: &mut Vec<CheckFinding>,
) {
    let mut seen = BTreeSet::new();
    for capability in &resolved.worker.capabilities {
        let key = format!("{}:{}", capability.role, capability.repo);
        if !seen.insert(key) {
            continue;
        }
        let Ok(repo) = RepoPath::parse(&capability.repo) else {
            findings.push(CheckFinding::online_error(
                "worker",
                CheckCategory::Config,
                format!(
                    "worker capability `{}` has an invalid repository path for online Forgejo validation",
                    capability.repo
                ),
            ));
            continue;
        };
        add_role_repo_forge_checks(
            resolved,
            client,
            base_url,
            &capability.role,
            std::slice::from_ref(&repo),
            "worker capability",
            findings,
        );
    }
}

fn add_role_repo_forge_checks(
    resolved: &Resolved,
    client: &BlockingHttpClient,
    base_url: &str,
    role: &str,
    repos: &[RepoPath],
    source: &str,
    findings: &mut Vec<CheckFinding>,
) {
    let Some(identity) = resolved.forge.role_identities.get(role) else {
        findings.push(CheckFinding::online_error(
            "worker",
            CheckCategory::Auth,
            format!("role `{role}` ({source}) has no Forgejo token for online worker checks"),
        ));
        return;
    };
    let credential = format!("worker role `{role}` credential");
    if !probe_forge_auth(
        client,
        base_url,
        identity.token.expose_secret(),
        "worker",
        &credential,
        findings,
    ) {
        return;
    }
    for repo in repos {
        probe_repo_visible(
            client,
            base_url,
            identity.token.expose_secret(),
            "worker",
            &credential,
            repo,
            findings,
        );
    }
}

fn forge_base_url<'a>(
    resolved: &'a Resolved,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) -> Option<&'a str> {
    let Some(base_url) = resolved.forge.url.as_deref() else {
        findings.push(CheckFinding::online_error(
            scope,
            CheckCategory::Network,
            "Forgejo reachability cannot run: forge URL is unset",
        ));
        return None;
    };
    if let Err(message) = validate_http_base_url(base_url, "Forgejo base URL") {
        push_network_url_error(scope, message, findings);
        return None;
    }
    Some(base_url)
}

fn probe_forge_auth(
    client: &BlockingHttpClient,
    base_url: &str,
    token: &str,
    scope: &str,
    credential: &str,
    findings: &mut Vec<CheckFinding>,
) -> bool {
    match forge_get(client, base_url, "/user", token) {
        Ok(status) if (200..300).contains(&status) => true,
        Ok(401 | 403) => {
            findings.push(CheckFinding::online_error(
                scope,
                CheckCategory::Auth,
                format!("Forgejo authentication failed for {credential}"),
            ));
            false
        }
        Ok(404) => {
            findings.push(CheckFinding::online_error(
                scope,
                CheckCategory::Network,
                "Forgejo API was not found at the configured base URL (`/api/v1/user` returned 404)",
            ));
            false
        }
        Ok(status) => {
            findings.push(CheckFinding::online_error(
                scope,
                CheckCategory::Forge,
                format!("Forgejo user probe returned unexpected HTTP status {status}"),
            ));
            false
        }
        Err(error) => {
            findings.push(CheckFinding::online_error(
                scope,
                CheckCategory::Network,
                format!("failed to reach configured Forgejo base URL: {error}"),
            ));
            false
        }
    }
}

fn probe_repo_visible(
    client: &BlockingHttpClient,
    base_url: &str,
    token: &str,
    scope: &str,
    credential: &str,
    repo: &RepoPath,
    findings: &mut Vec<CheckFinding>,
) {
    let repo_display = repo.display();
    let path = format!(
        "/repos/{}/{}",
        encode_path_segment(&repo.owner),
        encode_path_segment(&repo.name)
    );
    match forge_get(client, base_url, &path, token) {
        Ok(status) if (200..300).contains(&status) => {}
        Ok(401 | 403) => findings.push(CheckFinding::online_error(
            scope,
            CheckCategory::Auth,
            format!(
                "Forgejo authentication failed for {credential} while reading repository `{repo_display}`"
            ),
        )),
        Ok(404) => findings.push(CheckFinding::online_error(
            scope,
            CheckCategory::Repository,
            format!("repository `{repo_display}` is not visible/readable to {credential}"),
        )),
        Ok(status) => findings.push(CheckFinding::online_error(
            scope,
            CheckCategory::Forge,
            format!("repository `{repo_display}` probe returned unexpected HTTP status {status}"),
        )),
        Err(error) => findings.push(CheckFinding::online_error(
            scope,
            CheckCategory::Network,
            format!("failed to reach Forgejo while reading repository `{repo_display}`: {error}"),
        )),
    }
}

fn forge_get(
    client: &BlockingHttpClient,
    base_url: &str,
    path: &str,
    token: &str,
) -> Result<u16, String> {
    let url = api_url(base_url, path)?;
    let response = client.get(
        url,
        vec![
            ("Authorization".to_string(), format!("token {token}")),
            ("Accept".to_string(), "application/json".to_string()),
        ],
    )?;
    Ok(response.status)
}

fn api_url(base_url: &str, path: &str) -> Result<String, String> {
    validate_http_base_url(base_url, "Forgejo base URL")?;
    Ok(format!(
        "{}/api/v1{}",
        base_url.trim().trim_end_matches('/'),
        path
    ))
}

fn encode_path_segment(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
    out
}
