//! Top-level runner for `temper provision-forgejo`: drives provisioning/seeding.

use temper_reference_delivery::{DEFAULT_INTAKE_BODY, DEFAULT_INTAKE_TITLE};
use temper_workflow::{IntakeAuthor, RoleId};

use crate::provision::{self, ProvisionOptions};

use super::model::{ProvisionArgs, RunError};

fn build_runtime() -> Result<temper_engine_io::EngineRuntime, RunError> {
    temper_engine_io::build_runtime().map_err(RunError::Runtime)
}

fn intake_seed_from_args(args: &ProvisionArgs) -> Result<provision::IntakeIssueSeed, RunError> {
    Ok(provision::IntakeIssueSeed {
        title: args
            .intake_title
            .clone()
            .unwrap_or_else(|| DEFAULT_INTAKE_TITLE.into()),
        body: match &args.intake_body_file {
            Some(path) => std::fs::read_to_string(path)?,
            None => DEFAULT_INTAKE_BODY.into(),
        },
    })
}

pub fn run(args: &ProvisionArgs) -> Result<String, RunError> {
    if args.seed_only {
        return run_seed_only(args);
    }
    let runtime = build_runtime()?;
    let workflow = temper_reference_delivery::resolve_workflow(args.workflow_file.as_ref())?;
    let intake_seed = if args.seed_intake {
        Some(intake_seed_from_args(args)?)
    } else {
        None
    };
    let provision_args = args.clone();
    let provision_workflow = workflow;
    let (provisioned, issue) = temper_engine_io::runtime::block_on_runtime_with(
        &runtime,
        move |cx, _handle| async move {
            provision::provision_and_seed(
                &cx,
                &provision_args.base_url,
                &provision_args.admin_token,
                &provision_args.owner,
                &provision_args.name,
                provision_args.webhook_url.as_deref(),
                provision_args.webhook_secret_file.as_deref(),
                intake_seed.as_ref(),
                &provision_workflow,
                ProvisionOptions {
                    existing_repo: provision_args.existing_repo,
                    access: provision_args.access,
                },
            )
            .await
        },
    )?;
    provision::write_secrets_file(&args.out, &provision::format_secrets_env(&provisioned))?;
    let intake = issue
        .map(|number| format!("intake issue #{number}"))
        .unwrap_or_else(|| "no intake issue seeded".to_string());
    Ok(format!(
        "provisioned {}/{}: {} role(s), {}; secrets written to {}",
        provisioned.owner,
        provisioned.name,
        provisioned.roles.len(),
        intake,
        args.out.display(),
    ))
}

/// Files only the intake issue, assuming an earlier `--seed-intake no` pass
/// already provisioned the org/users/repo/labels/CI/webhook and wrote `--out`.
///
/// This lets a launcher start the workers (and the wake trigger) first, then
/// file the entry issue so its creation webhook proves the wake path — instead
/// of the workers only discovering a pre-seeded issue on their next poll.
fn run_seed_only(args: &ProvisionArgs) -> Result<String, RunError> {
    let runtime = build_runtime()?;
    let workflow = temper_reference_delivery::resolve_workflow(args.workflow_file.as_ref())?;
    let seed = intake_seed_from_args(args)?;
    let token = match workflow.intake_author() {
        Some(IntakeAuthor::SiteAdmin) => args.admin_token.clone(),
        Some(IntakeAuthor::Role(role)) => provision::role_token_from_secrets_file(&args.out, role)?,
        None => provision::role_token_from_secrets_file(&args.out, &RoleId::new("human"))?,
    };
    let seed_args = args.clone();
    let number = temper_engine_io::runtime::block_on_runtime_with(
        &runtime,
        move |_cx, _handle| async move {
            use temper_forge::RepositoryPath;
            use temper_forge::config::ForgejoConfig;

            let config = ForgejoConfig::new(&seed_args.base_url, &token)
                .with_default_repo(&seed_args.owner, &seed_args.name);
            let forge = temper_forge::factory::new_forgejo(config);
            let repo = forge
                .get_repository_by_path(&RepositoryPath::new(&seed_args.owner, &seed_args.name))
                .await?
                .ok_or_else(|| provision::ProvisionError::Shape {
                    what: "repository".into(),
                    detail: format!(
                        "{}/{} not readable when seeding intake issue",
                        seed_args.owner, seed_args.name
                    ),
                })?;
            provision::seed_intake_issue(forge.as_ref(), &repo.id, &seed, &workflow).await
        },
    )?;
    Ok(format!(
        "seeded {}/{}: intake issue #{number}",
        args.owner, args.name,
    ))
}
