//! Writing and reading the live-credentials file (`--out`).
//!
//! The demo provision-forgejo CLI writes the provisioned secrets in the same
//! `credentials.toml` format the runtime actually reads (`[forge.users.<role>]`
//! with `user`/`email`/`password`/`token`, plus a `bot` automation user). This
//! is the file the daemon loads via `--credentials`; there is no separate env
//! file. `temper init` writes its own `credentials.toml` and never touches this.

use std::collections::BTreeMap;
use std::path::Path;

use temper_config::{
    Credentials, ForgeCredentials, ProvisionedForgeUser, SCHEMA_VERSION,
    forge_users_from_provisioned,
};
use temper_workflow::RoleId;

use super::{ProvisionError, Provisioned, Result};

/// Builds the `credentials.toml` document for a provisioning result: each role
/// identity under `[forge.users.<role>]`, plus the automation account under its
/// own login (`[forge.users.bot]`), so `[forge] ci_user = "bot"` resolves to it.
pub fn credentials_document(provisioned: &Provisioned) -> Credentials {
    let mut provisioned_users: BTreeMap<String, ProvisionedForgeUser> = BTreeMap::new();
    for (role, identity) in &provisioned.roles {
        provisioned_users.insert(
            role.as_str().to_string(),
            ProvisionedForgeUser {
                user: Some(identity.user.clone()),
                email: Some(identity.email.clone()),
                password: Some(identity.password.clone()),
                token: Some(identity.token.clone()),
            },
        );
    }
    // The automation (bot) identity, keyed by its login so the daemon's
    // `[forge] ci_user` (set to `bot`) resolves to it.
    let bot = &provisioned.automation;
    provisioned_users.insert(
        bot.user.clone(),
        ProvisionedForgeUser {
            user: Some(bot.user.clone()),
            email: Some(bot.email.clone()),
            password: Some(bot.password.clone()),
            token: Some(bot.token.clone()),
        },
    );

    Credentials {
        schema_version: SCHEMA_VERSION,
        forge: ForgeCredentials {
            users: forge_users_from_provisioned(&provisioned_users),
        },
        agent: Default::default(),
    }
}

/// Writes the provisioned [`credentials_document`] to `path` (chmod 600 on Unix,
/// validated round-trip), creating parent directories as needed. `force`
/// overwrites an existing file (a provision re-run always rewrites).
pub fn write_credentials_file(path: &Path, document: &Credentials) -> Result<()> {
    temper_config::write_credentials(document, path, true).map_err(|error| ProvisionError::Shape {
        what: "credentials file".into(),
        detail: error.to_string(),
    })
}

/// Reads a provisioned role's minted token back out of a credentials file
/// written by [`write_credentials_file`].
///
/// This supports seed-only provisioning runs, where the entry issue is filed in
/// a second pass (after the workers are up) and the authoring role's token must
/// be recovered from the credentials file produced by the earlier provision
/// pass.
pub fn role_token_from_secrets_file(path: &Path, role: &RoleId) -> Result<String> {
    let credentials = Credentials::load(path).map_err(|error| ProvisionError::Shape {
        what: "intake seed author".into(),
        detail: error.to_string(),
    })?;
    credentials
        .forge
        .users
        .get(role.as_str())
        .and_then(|user| user.token.clone())
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| ProvisionError::Shape {
            what: "intake seed author".into(),
            detail: format!(
                "no token for role `{role}` found in {} (expected [forge.users.{role}] token)",
                path.display()
            ),
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use temper_forge::RepositoryId;
    use temper_forge::config::FORGEJO_ROLE_PASSWORD as ROLE_PASSWORD;
    use temper_provision::{BOT_USER, Provisioned, RoleIdentity};
    use temper_workflow::RoleId;

    use super::*;

    fn sample(roles: BTreeMap<RoleId, RoleIdentity>) -> Provisioned {
        Provisioned {
            owner: "acme".into(),
            name: "service".into(),
            repository: RepositoryId::new("r"),
            roles,
            automation: RoleIdentity {
                user: BOT_USER.into(),
                email: "bot@example.invalid".into(),
                token: "bot-tok".into(),
                password: "bot-pw".into(),
            },
        }
    }

    #[test]
    fn role_token_round_trips_through_credentials_file() {
        let mut roles = BTreeMap::new();
        roles.insert(
            RoleId::new("code-reviewer"),
            RoleIdentity {
                user: "reviewer".into(),
                email: "reviewer@example.invalid".into(),
                token: "tok-secret".into(),
                password: ROLE_PASSWORD.into(),
            },
        );
        let document = credentials_document(&sample(roles));
        let dir = std::env::temp_dir().join(format!("temper-seed-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("credentials.toml");
        write_credentials_file(&path, &document).expect("write credentials file");

        let token = role_token_from_secrets_file(&path, &RoleId::new("code-reviewer"))
            .expect("recovers the role token");
        assert_eq!(token, "tok-secret");

        let missing = role_token_from_secrets_file(&path, &RoleId::new("architect"))
            .expect_err("absent role errors");
        assert!(matches!(missing, ProvisionError::Shape { .. }));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn credentials_document_carries_roles_and_bot() {
        let mut roles = BTreeMap::new();
        roles.insert(
            RoleId::new("code-reviewer"),
            RoleIdentity {
                user: "reviewer".into(),
                email: "reviewer@example.invalid".into(),
                token: "tok".into(),
                password: "pw".into(),
            },
        );
        let document = credentials_document(&sample(roles));
        let reviewer = document
            .forge
            .users
            .get("code-reviewer")
            .expect("reviewer user present");
        assert_eq!(reviewer.user.as_deref(), Some("reviewer"));
        assert_eq!(reviewer.token.as_deref(), Some("tok"));
        assert_eq!(reviewer.password.as_deref(), Some("pw"));

        let bot = document
            .forge
            .users
            .get(BOT_USER)
            .expect("bot user present");
        assert_eq!(bot.token.as_deref(), Some("bot-tok"));
        assert_eq!(bot.password.as_deref(), Some("bot-pw"));

        // The serialized form is real `credentials.toml` the runtime reads.
        let text = toml::to_string(&document).expect("serializes");
        assert!(text.contains("[forge.users.code-reviewer]"));
        assert!(text.contains("[forge.users.bot]"));
    }
}
