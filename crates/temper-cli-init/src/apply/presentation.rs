// SPDX-License-Identifier: MPL-2.0

//! Human presentation for `temper apply`.

use std::path::Path;

use temper_cli_common::Prompter;

use crate::deployment::DeploymentBundle;

use super::args::ApplyCredentialMode;

pub(super) fn show_apply_plan(
    p: &mut dyn Prompter,
    bundle: &DeploymentBundle,
    credential_mode: ApplyCredentialMode,
    credential_path: Option<&Path>,
) {
    p.note("Apply plan:");
    p.note(&format!("  forge: {}", bundle.forge.base_url));
    match (&bundle.metadata.name, &bundle.metadata.topology) {
        (Some(name), Some(topology)) => p.note(&format!("  deployment: {name} ({topology})")),
        (Some(name), None) => p.note(&format!("  deployment: {name}")),
        (None, Some(topology)) => p.note(&format!("  topology: {topology}")),
        (None, None) => {}
    }
    p.note(&format!("  workflow: {}", bundle.metadata.workflow_source));
    p.note(&format!(
        "  repositories: {} repo(s)",
        bundle.repositories.len()
    ));
    for desired in &bundle.repositories {
        let plan = &desired.plan;
        let mode = if plan.existing_repo {
            "require existing repository"
        } else {
            "create if missing"
        };
        let webhook = if bundle.webhook.is_some() {
            "yes"
        } else {
            "no"
        };
        p.note(&format!(
            "  - {}/{}: {mode}, branch {}, {} role(s), {} label(s), webhook {webhook}",
            plan.repo.owner,
            plan.repo.name,
            plan.default_branch,
            plan.roles.len(),
            plan.labels.len(),
        ));
    }
    if bundle
        .repositories
        .iter()
        .any(|repository| repository.plan.existing_repo)
    {
        p.note("  --existing-repo compatibility: applies to every configured repository");
    }
    match (credential_mode, credential_path) {
        (ApplyCredentialMode::UpdateLocalCredentials, Some(path)) => p.note(&format!(
            "  credentials: update {} after success",
            path.display()
        )),
        (ApplyCredentialMode::UpdateLocalCredentials, None) => {
            p.note("  credentials: no durable update target")
        }
        (ApplyCredentialMode::SkipLocalCredentials, _) => p.note("  credentials: not modified"),
    }
    if bundle.metadata.worker_pools > 0 || bundle.metadata.agent_profiles > 0 {
        p.note(&format!(
            "  metadata: {} worker pool(s), {} agent profile(s)",
            bundle.metadata.worker_pools, bundle.metadata.agent_profiles,
        ));
    }
}
