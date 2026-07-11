// SPDX-License-Identifier: MPL-2.0

//! Loading and validation for the canonical deployment bundle.

use std::path::Path;

use temper_config::{
    LoadInputs, LoadOptions, LoadedDocuments, PathResolver, Secret,
    load_documents_explicit_with_secret_validation,
};

use crate::InitError;

use super::model::{
    DeploymentBundle, DeploymentMetadata, DesiredRepository, DesiredWebhook, ForgeAuthentication,
};

/// Loads one deployment for plan/apply, validating the workflow once and
/// constructing desired state for every resolved repository.
pub fn load_deployment(
    options: &LoadOptions,
    env: &temper_config::EnvMap,
    paths: &PathResolver,
    existing_repo: bool,
) -> Result<DeploymentBundle, InitError> {
    let LoadedDocuments {
        config,
        credentials,
        resolved,
        loaded,
        credential_source,
    } = load_documents_explicit_with_secret_validation(
        &LoadInputs {
            explicit_config: options.config.clone(),
            explicit_credentials: options.credentials.clone(),
            env,
            paths,
        },
        // Fresh init bundles can reference the admin token minted by this apply.
        // Runtime/check callers retain strict validation.
        false,
    )
    .map_err(|error| InitError::Path(format!("load deployment: {error}")))?;

    let base_url = resolved.forge.url.clone().ok_or_else(|| {
        InitError::Unsupported("deployment requires `[forge] url` in config.toml".to_string())
    })?;
    if resolved.engine.repos.is_empty() {
        return Err(InitError::Unsupported(
            "deployment requires at least one `[engine] repos` entry".to_string(),
        ));
    }

    let admin_key = non_empty(config.forge.admin.as_deref());
    let admin = admin_key
        .as_ref()
        .and_then(|key| credentials.forge.users.get(key));
    let admin_user = admin_key.as_ref().map(|key| {
        admin
            .and_then(|user| non_empty(user.user.as_deref()))
            .unwrap_or_else(|| key.clone())
    });
    let admin_password = admin
        .and_then(|user| non_empty(user.password.as_deref()))
        .map(Secret::from);
    // Authentication is optional in the canonical desired-state bundle. Plan
    // can still render every repository with an explicit unavailable
    // inspection, while apply validates the credentials at its mutating adapter
    // boundary.
    let admin_token = resolved.forge.admin_token.clone();

    let (workflow, workflow_source) = match &resolved.engine.workflow_file {
        Some(path) => (
            temper_reference_delivery::load_workflow(path).map_err(|error| {
                InitError::Unsupported(format!("load workflow {}: {error}", path.display()))
            })?,
            path.display().to_string(),
        ),
        None => (
            temper_reference_delivery::basic_delivery_workflow(),
            "built-in basic-delivery".to_string(),
        ),
    };

    let webhook_url = format!("http://{}/forgejo/webhook", resolved.engine.bind);
    let webhook = if let Some(secret) = resolved.engine.webhook_secret_value.as_ref() {
        Some(DesiredWebhook {
            url: webhook_url,
            secret: secret.clone(),
            events: temper_forge::WebhookEvents::All,
        })
    } else {
        resolved
            .engine
            .webhook_secret_file
            .as_ref()
            .map(|path| load_webhook(path, &webhook_url))
            .transpose()?
    };

    let repositories = resolved
        .engine
        .repos
        .iter()
        .map(|repo| {
            DesiredRepository::from_workflow(&workflow, &repo.owner, &repo.name, existing_repo)
                .map_err(InitError::Unsupported)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let metadata = DeploymentMetadata {
        name: resolved.deployment.name.clone(),
        topology: resolved
            .deployment
            .topology
            .map(|topology| topology.as_str().to_string()),
        workflow_source,
        worker_pools: resolved.worker.pools.len(),
        agent_profiles: resolved.agent.profiles.len(),
    };

    Ok(DeploymentBundle {
        config,
        credentials,
        resolved,
        loaded,
        credential_source,
        metadata,
        workflow,
        forge: ForgeAuthentication {
            base_url,
            admin_user,
            admin_password,
            admin_token,
        },
        webhook,
        repositories,
        admin_key,
    })
}

fn load_webhook(path: &Path, webhook_url: &str) -> Result<DesiredWebhook, InitError> {
    let secret = std::fs::read_to_string(path).map_err(|error| {
        InitError::Path(format!("read webhook secret {}: {error}", path.display()))
    })?;
    Ok(DesiredWebhook {
        url: webhook_url.to_string(),
        secret: Secret::from(secret.trim().to_string()),
        events: temper_forge::WebhookEvents::All,
    })
}

pub(crate) fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_debug_redacts_auth_and_webhook_secrets() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.toml"),
            "schema_version = 1\n[forge]\nurl = \"http://forge.local\"\nadmin = \"root\"\n[engine]\nrepos = [\"acme/service\"]\nroles = [\"engineer\"]\nwebhook_secret = \"hook\"\n",
        )
        .expect("config");
        std::fs::write(
            dir.path().join("credentials.toml"),
            "schema_version = 1\n[forge.users.root]\npassword = \"super-password\"\n[secrets]\nhook = \"super-webhook\"\n",
        )
        .expect("credentials");
        let bundle = load_deployment(
            &LoadOptions {
                config: Some(dir.path().to_path_buf()),
                credentials: None,
            },
            &temper_config::EnvMap::new(),
            &PathResolver::default(),
            false,
        )
        .expect("bundle");

        let debug = format!("{bundle:?}");
        assert!(!debug.contains("super-password"), "{debug}");
        assert!(!debug.contains("super-webhook"), "{debug}");
        assert!(debug.contains("REDACTED"), "{debug}");
    }
}
