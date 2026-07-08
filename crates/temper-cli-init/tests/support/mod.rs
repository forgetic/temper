// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::path::Path;

use temper_cli_common::LoadOptions;
use temper_cli_init::{ProvisionOutcome, ProvisionRequest, Provisioner};
use temper_config::{NoEnv, ResolveOptions};
use temper_forge::RepositoryId;
use temper_provision::{Provisioned, RoleIdentity};
use temper_workflow::RoleId;

/// Returns a canned `Provisioned` with two role identities (architect,
/// engineer) + a `bot` automation identity, and records the request it was
/// handed so tests can assert the wiring.
pub struct StubProvisioner {
    pub seen: Option<ProvisionRequest>,
}

impl Provisioner for StubProvisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String> {
        self.seen = Some(request.clone());
        let identity = |user: &str| RoleIdentity {
            user: user.to_string(),
            email: format!("{user}@example.invalid"),
            token: format!("token-{user}"),
            password: format!("pw-{user}"),
        };
        let mut roles = BTreeMap::new();
        roles.insert(RoleId::new("architect"), identity("architect"));
        roles.insert(RoleId::new("engineer"), identity("engineer"));
        let provisioned = Provisioned {
            owner: request.owner.clone(),
            name: request.name.clone(),
            repository: RepositoryId::new(format!("{}/{}", request.owner, request.name)),
            roles,
            automation: identity("bot"),
        };
        Ok(ProvisionOutcome {
            provisioned,
            admin_token: "admin-rest-token".to_string(),
        })
    }
}

#[allow(dead_code)]
pub fn resolve_generated_bundle_non_strict(
    config_path: &Path,
    credentials_path: &Path,
) -> temper_config::Resolved {
    let config = temper_config::Config::load(config_path).expect("config parses");
    let credentials =
        temper_config::Credentials::load(credentials_path).expect("credentials parse");
    let mut options = config_path
        .parent()
        .map(ResolveOptions::from_config_base_dir)
        .unwrap_or_default();
    options.validate_secret_references = false;
    temper_config::resolve_with_options(&config, &credentials, &NoEnv, &options)
        .expect("generated bundle resolves non-strictly")
}

#[allow(dead_code)]
pub fn load_generated_bundle(
    config_path: &Path,
    credentials_path: &Path,
) -> temper_config::Resolved {
    temper_config::load_with_env(
        &LoadOptions {
            config: Some(config_path.to_path_buf()),
            credentials: Some(credentials_path.to_path_buf()),
        },
        &NoEnv,
    )
    .expect("generated bundle parses and resolves")
    .0
}
