// SPDX-License-Identifier: MPL-2.0

//! Audited credential-update policy for deployment apply.

use std::collections::BTreeMap;
use std::path::PathBuf;

use temper_config::{
    CredentialSourceOrigin, Credentials, ExposeSecret, ProvisionedForgeUser, SCHEMA_VERSION,
    Secret, forge_users_from_provisioned,
};
use temper_provision::Provisioned;

use crate::{InitError, write};

use super::DeploymentBundle;

/// Resolves the durable TOML target selected by the loader.
///
/// Ambient systemd credential directories are intentionally read-only. An
/// explicit directory is durable by operator opt-in and maps to its
/// `credentials.toml`, leaving all named files untouched.
pub fn durable_credentials_path(bundle: &DeploymentBundle) -> Result<PathBuf, InitError> {
    let source = bundle.credential_source.as_ref().ok_or_else(|| {
        InitError::Unsupported(
            "cannot update credentials without a durable target; pass `--secrets <FILE|DIR>`"
                .to_string(),
        )
    })?;
    if source.origin == CredentialSourceOrigin::CredentialsDirectory {
        return Err(InitError::Unsupported(
            "CREDENTIALS_DIRECTORY is read-only for `temper apply`; pass `--secrets <FILE|DIR>` \
             to select a durable credential target, or use the programmatic no-write mode"
                .to_string(),
        ));
    }
    Ok(source.credentials_file())
}

/// Merges successful provisioning output into the parsed credentials document.
/// Raw values are exposed only here, immediately before the audited
/// `temper_config::write_credentials` schema boundary.
pub fn merge_provisioned_credentials(
    credentials: &mut Credentials,
    admin_key: Option<&str>,
    provisioned: &[Provisioned],
    admin_token: &Secret,
) {
    // A directory made only of systemd-style named files has no TOML document
    // to contribute a schema version. The first durable credentials.toml we
    // create must still be a valid current-schema document.
    credentials.schema_version = SCHEMA_VERSION;
    for repository in provisioned {
        for (key, user) in role_and_bot_users(repository) {
            credentials.forge.users.insert(key, user);
        }
    }

    let token = admin_token.expose_secret().to_string();
    if let Some(admin_key) = admin_key {
        credentials
            .forge
            .users
            .entry(admin_key.to_string())
            .or_default()
            .token = Some(token.clone());
    }
    write::add_forge_engine_token_secret(credentials, token);
}

fn role_and_bot_users(provisioned: &Provisioned) -> BTreeMap<String, temper_config::ForgeUser> {
    let mut users = BTreeMap::new();
    for (role, identity) in &provisioned.roles {
        users.insert(
            role.as_str().to_string(),
            ProvisionedForgeUser {
                user: Some(identity.user.clone()),
                email: Some(identity.email.clone()),
                token: Some(identity.token.clone()),
            },
        );
    }
    let bot = &provisioned.automation;
    users.insert(
        bot.user.clone(),
        ProvisionedForgeUser {
            user: Some(bot.user.clone()),
            email: Some(bot.email.clone()),
            token: Some(bot.token.clone()),
        },
    );
    forge_users_from_provisioned(&users)
}
