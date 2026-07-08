// SPDX-License-Identifier: MPL-2.0

mod forge;
mod http;
mod provider;

use temper_cli_common::{EnvMap, PathResolver};
use temper_config::Resolved;

use super::finding::{CheckCategory, CheckFinding};
use super::options::{CheckOptions, Component};

pub(super) fn add_online_findings(
    resolved: &Resolved,
    env: &EnvMap,
    paths: &PathResolver,
    options: &CheckOptions,
    findings: &mut Vec<CheckFinding>,
) {
    match options.component {
        Component::Standalone => {
            forge::add_engine_forge_checks(resolved, findings);
            provider::add_standalone_provider_checks(resolved, env, paths, findings);
        }
        Component::Engine => forge::add_engine_forge_checks(resolved, findings),
        Component::Worker => {
            provider::add_worker_provider_checks(
                resolved,
                env,
                paths,
                options.pool.as_deref(),
                findings,
            );
            forge::add_worker_forge_visibility_checks(resolved, options.pool.as_deref(), findings);
        }
    }
}

fn validate_http_base_url(raw: &str, label: &str) -> Result<(), String> {
    let value = raw.trim();
    if value.is_empty() || value != raw {
        return Err(format!(
            "{label} must be a non-empty URL without surrounding whitespace"
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{label} must not contain whitespace"));
    }
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .ok_or_else(|| format!("{label} must start with http:// or https://"))?;
    let host_end = rest.find(&['/', '?', '#'][..]).unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() {
        return Err(format!("{label} must include a host"));
    }
    Ok(())
}

fn push_network_url_error(scope: &str, message: String, findings: &mut Vec<CheckFinding>) {
    findings.push(CheckFinding::online_error(
        scope,
        CheckCategory::Network,
        message,
    ));
}
