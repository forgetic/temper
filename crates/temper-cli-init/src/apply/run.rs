// SPDX-License-Identifier: MPL-2.0

//! Apply orchestration over the canonical deployment bundle.

use temper_cli_common::Prompter;

use crate::InitError;
use crate::deployment::{durable_credentials_path, load_deployment, merge_provisioned_credentials};
use crate::provisioner::{ApplyPlanRequest, ApplyProvisioner};

use super::args::{ApplyCredentialMode, ApplyOptions};
use super::presentation::show_apply_plan;

/// Loads, presents, and applies one deployment. Credential bytes change only
/// after every desired repository has succeeded.
pub fn run_apply(
    p: &mut dyn Prompter,
    provisioner: &mut dyn ApplyProvisioner,
    opts: &ApplyOptions,
) -> Result<(), InitError> {
    let bundle = load_deployment(&opts.options, &opts.env, &opts.paths, opts.existing_repo)?;

    // Enforce read-only ambient credentials before confirmation and, crucially,
    // before constructing or invoking any mutating Forge adapter call.
    let credential_path = match opts.credential_mode {
        ApplyCredentialMode::SkipLocalCredentials => None,
        ApplyCredentialMode::UpdateLocalCredentials => Some(durable_credentials_path(&bundle)?),
    };
    show_apply_plan(p, &bundle, opts.credential_mode, credential_path.as_deref());

    if !opts.yes
        && !p.confirm(
            &format!(
                "Apply this provisioning plan to {} repo(s) on {}?",
                bundle.repositories.len(),
                bundle.forge.base_url,
            ),
            false,
        )?
    {
        p.note("Skipped forge provisioning at operator confirmation.");
        return Ok(());
    }

    let request = ApplyPlanRequest {
        base_url: bundle.forge.base_url.clone(),
        admin_user: bundle.forge.admin_user.clone(),
        admin_password: bundle.forge.admin_password.clone(),
        admin_token: bundle.forge.admin_token.clone(),
        plans: bundle.expose_provision_plans(),
    };
    let outcome = provisioner
        .provision_apply_plan(&request)
        .map_err(InitError::Provision)?;
    if outcome.provisioned.len() != bundle.repositories.len() {
        return Err(InitError::Provision(format!(
            "provisioner returned {} result(s) for {} repo plan(s)",
            outcome.provisioned.len(),
            bundle.repositories.len()
        )));
    }

    if let Some(credentials_path) = credential_path {
        let mut credentials = bundle.credentials;
        merge_provisioned_credentials(
            &mut credentials,
            bundle.admin_key.as_deref(),
            &outcome.provisioned,
            &outcome.admin_token,
        );
        temper_config::write_credentials(&credentials, &credentials_path, true)
            .map_err(|error| InitError::Write(error.to_string()))?;
        p.note(&format!(
            "Updated {} (chmod 600)",
            credentials_path.display()
        ));
    } else {
        p.note("Local credentials were not modified.");
    }

    p.note(&format!(
        "Provisioned {} repo(s) on {}.",
        outcome.provisioned.len(),
        bundle.forge.base_url,
    ));
    for provisioned in &outcome.provisioned {
        p.note(&format!(
            "  - {}/{}: {} role(s), automation bot `{}`",
            provisioned.owner,
            provisioned.name,
            provisioned.roles.len(),
            provisioned.automation.user,
        ));
    }
    p.note("Now run `temper serve standalone` to start the engine, worker, and agent.");
    Ok(())
}
