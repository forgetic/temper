// SPDX-License-Identifier: MPL-2.0

use std::io::ErrorKind;
use std::path::Path;

use temper_config::{
    AgentProfileSettings, ProviderCredential, Resolved, SecretReference, WorkerPoolSettings,
};

use super::finding::{CheckCategory, CheckFinding};
use super::options::{CheckOptions, Component};

pub(super) fn scoped_findings(resolved: &Resolved, options: &CheckOptions) -> Vec<CheckFinding> {
    let mut findings = Vec::new();
    match options.component {
        Component::Standalone => {
            add_engine_findings(resolved, &mut findings);
            add_worker_findings(resolved, None, &mut findings);
        }
        Component::Engine => add_engine_findings(resolved, &mut findings),
        Component::Worker => add_worker_findings(resolved, options.pool.as_deref(), &mut findings),
        Component::Trigger => findings.push(error_finding(
            "trigger component checks are not implemented yet",
        )),
    }
    findings
}

pub(super) fn add_offline_findings(
    resolved: &Resolved,
    options: &CheckOptions,
    findings: &mut Vec<CheckFinding>,
) {
    add_workflow_file_finding(resolved, findings);
    add_path_findings(resolved, options.component, findings);
}

fn add_engine_findings(resolved: &Resolved, findings: &mut Vec<CheckFinding>) {
    if resolved.forge.url.is_none() {
        findings.push(config_error(
            "engine",
            "forge URL is unset (set `[forge] url` in temper.toml)",
        ));
    }
    add_missing_secret(
        "engine.forge_token",
        resolved.engine.forge_token.as_ref(),
        findings,
    );
    if resolved.engine.forge_token.is_none() && resolved.forge.admin_token.is_none() {
        findings.push(config_error(
            "engine",
            "forge admin token is unset (set `[engine] forge_token` to a named secret, or set a `token` under \
             `[forge.users.<admin>]` in credentials.toml and name the admin via `[forge] admin`)",
        ));
    }
    if resolved.engine.repos.is_empty() {
        findings.push(config_error(
            "engine",
            "no repositories configured (`[engine] repos`)",
        ));
    }
    if resolved.engine.roles.is_empty() {
        findings.push(config_error(
            "engine",
            "no roles configured (`[engine] roles`)",
        ));
    }
    add_missing_secret(
        "engine.webhook_secret",
        resolved.engine.webhook_secret.as_ref(),
        findings,
    );
    if resolved.forge.web_ui.is_none() {
        findings.push(CheckFinding::offline_note(
            "engine",
            CheckCategory::Forge,
            "no CI-reader web-UI credentials (`[forge] ci_user` + that user's password); \
             CI status reads will fail on a REST-less Forgejo (ADR 0019)",
        ));
    }
}

fn add_worker_findings(resolved: &Resolved, pool: Option<&str>, findings: &mut Vec<CheckFinding>) {
    if resolved.worker.git_base_url.is_none() && resolved.forge.url.is_none() {
        findings.push(config_error(
            "worker",
            "worker git base URL is unset (set `[worker] git_base_url` or `[forge] url`)",
        ));
    }
    match pool {
        Some(name) => match resolved.worker.pools.iter().find(|pool| pool.name == name) {
            Some(pool) => add_pool_findings(resolved, pool, findings),
            None => findings.push(config_error(
                "worker",
                format!("worker pool `{name}` is not configured"),
            )),
        },
        None => {
            if resolved.worker.capabilities.is_empty() && resolved.worker.pools.is_empty() {
                findings.push(config_error(
                    "worker",
                    "no worker capabilities or pools configured (`[worker] capabilities` or `[[worker.pools]]`)",
                ));
            }
            for capability in &resolved.worker.capabilities {
                add_role_identity_finding(resolved, &capability.role, &capability.repo, findings);
            }
            for pool in &resolved.worker.pools {
                add_pool_findings(resolved, pool, findings);
            }
            if resolved.worker.pools.is_empty() {
                add_agent_provider_note(resolved, findings);
            }
        }
    }
}

fn add_pool_findings(
    resolved: &Resolved,
    pool: &WorkerPoolSettings,
    findings: &mut Vec<CheckFinding>,
) {
    if pool.repos.is_empty() {
        findings.push(config_error(
            "worker",
            format!("worker pool `{}` must declare at least one repo", pool.name),
        ));
    }
    if pool.max_concurrent_jobs.is_none() {
        findings.push(config_error(
            "worker",
            format!(
                "worker pool `{}` must set max_concurrent_jobs for runtime capacity policy",
                pool.name
            ),
        ));
    }
    add_missing_secret(
        &format!("worker pool `{}` worker_token", pool.name),
        pool.worker_token.as_ref(),
        findings,
    );
    for role in &pool.roles {
        add_role_identity_finding(resolved, role, &format!("pool `{}`", pool.name), findings);
    }
    match pool.agent_profile.as_deref() {
        Some(profile_name) => match resolved.agent.profiles.get(profile_name) {
            Some(profile) => add_profile_credential_finding(profile_name, profile, findings),
            None => findings.push(config_error(
                "worker",
                format!(
                    "worker pool `{}` references missing agent profile `{profile_name}`",
                    pool.name
                ),
            )),
        },
        None => add_agent_provider_note(resolved, findings),
    }
}

fn add_profile_credential_finding(
    name: &str,
    profile: &AgentProfileSettings,
    findings: &mut Vec<CheckFinding>,
) {
    match profile.credential.as_ref() {
        Some(reference) => add_missing_secret(
            &format!("agent profile `{name}` credential"),
            Some(reference),
            findings,
        ),
        None => findings.push(config_error(
            "provider",
            format!(
                "agent profile `{name}` has no credential configured (`agent.profiles.{name}.credential`)"
            ),
        )),
    }
}

fn add_role_identity_finding(
    resolved: &Resolved,
    role: &str,
    scope: &str,
    findings: &mut Vec<CheckFinding>,
) {
    if !resolved.forge.role_identities.contains_key(role) {
        findings.push(config_error(
            "worker",
            format!(
                "role `{role}` ({scope}) has no git identity: add `[forge.users.{role}]` with a token to the credentials file"
            ),
        ));
    }
}

fn add_agent_provider_note(resolved: &Resolved, findings: &mut Vec<CheckFinding>) {
    if matches!(
        resolved.agent.provider.credential,
        ProviderCredential::Ambient
    ) {
        findings.push(CheckFinding::offline_note(
            "provider",
            CheckCategory::Provider,
            format!(
                "agent provider `{}` has no credential in the credentials file (`[agent.providers.{}]`); \
             falling back to ambient auth (env / ~/.pi/agent/auth.json)",
                resolved.agent.provider.kind.as_str(),
                resolved.agent.provider.kind.as_str()
            ),
        ));
    }
}

fn add_missing_secret(
    field: &str,
    reference: Option<&SecretReference>,
    findings: &mut Vec<CheckFinding>,
) {
    if let Some(reference) = reference.filter(|reference| !reference.available) {
        findings.push(config_error(
            "secrets",
            format!("{field} references missing secret `{}`", reference.name),
        ));
    }
}

fn add_workflow_file_finding(resolved: &Resolved, findings: &mut Vec<CheckFinding>) {
    if let Some(path) = &resolved.paths.workflow_file {
        if let Err(error) = temper_workflow::load_workflow(path) {
            findings.push(CheckFinding::offline_error(
                "workflow",
                CheckCategory::Workflow,
                error.to_string(),
            ));
        }
    }
}

fn add_path_findings(resolved: &Resolved, component: Component, findings: &mut Vec<CheckFinding>) {
    match component {
        Component::Standalone => {
            add_engine_path_findings(resolved, findings);
            add_worker_path_findings(resolved, findings);
        }
        Component::Engine => add_engine_path_findings(resolved, findings),
        Component::Worker => add_worker_path_findings(resolved, findings),
        Component::Trigger => {}
    }
}

fn add_engine_path_findings(resolved: &Resolved, findings: &mut Vec<CheckFinding>) {
    if let Some(path) = &resolved.paths.state_dir {
        add_directory_finding("paths.state_dir", path, findings);
    }
    if let Some(path) = &resolved.engine.webhook_secret_file {
        add_file_finding("engine.webhook_secret_file", path, findings);
    }
}

fn add_worker_path_findings(resolved: &Resolved, findings: &mut Vec<CheckFinding>) {
    add_directory_finding(
        "paths.workspace_dir",
        &resolved.paths.workspace_dir,
        findings,
    );
    if let Some(path) = &resolved.agent.config_dir {
        add_directory_finding("agent.config_dir", path, findings);
    }
}

fn add_directory_finding(field: &str, path: &Path, findings: &mut Vec<CheckFinding>) {
    match path.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => findings.push(path_error(format!(
            "{field} path {} exists but is not a directory",
            path.display()
        ))),
        Err(error) if error.kind() == ErrorKind::NotFound => findings.push(path_note(format!(
            "{field} path {} does not exist yet",
            path.display()
        ))),
        Err(error) => findings.push(path_error(format!(
            "failed to inspect {field} path {}: {error}",
            path.display()
        ))),
    }
}

fn add_file_finding(field: &str, path: &Path, findings: &mut Vec<CheckFinding>) {
    match path.metadata() {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => findings.push(path_error(format!(
            "{field} path {} exists but is not a file",
            path.display()
        ))),
        Err(error) => findings.push(path_error(format!(
            "failed to read {field} path {}: {error}",
            path.display()
        ))),
    }
}

pub(super) fn error_finding(message: impl Into<String>) -> CheckFinding {
    config_error("config", message)
}

fn config_error(scope: impl Into<String>, message: impl Into<String>) -> CheckFinding {
    CheckFinding::offline_error(scope, CheckCategory::Config, message)
}

fn path_error(message: impl Into<String>) -> CheckFinding {
    CheckFinding::offline_error("paths", CheckCategory::Path, message)
}

fn path_note(message: impl Into<String>) -> CheckFinding {
    CheckFinding::offline_note("paths", CheckCategory::Path, message)
}
