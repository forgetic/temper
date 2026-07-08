// SPDX-License-Identifier: MPL-2.0

//! `temper apply` — run the forge-side provisioning plan for a deployment.
//!
//! The command loads the configured deployment, builds the same backend-agnostic
//! [`temper_provision::ProvisionPlan`] shape used by the init-local apply path,
//! shows that plan to the operator, then executes it for every configured
//! repository. Local credentials mutation is kept as an explicit compatibility
//! step after a successful provisioning run; declining or failing the plan leaves
//! `credentials.toml` untouched.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use temper_cli_common::{
    EX_USAGE, EnvMap, LoadOptions, PathResolver, Prompter, TerminalPrompter, resolve_targets,
};
use temper_config::{Config, Credentials, ExposeSecret, ResolveOptions};

use crate::provisioner::{
    ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner, ForgejoProvisioner, ProvisionOutcome,
    build_deployment_repo_plan,
};
use crate::{InitError, write};

/// `temper apply [OPTIONS]` usage.
pub const APPLY_USAGE: &str = "\
Apply a temper deployment plan to the forge.

Loads config.toml + credentials.toml, renders the configured Forgejo repos,
workflow labels, role users, and webhooks into a shared provisioning plan, shows
that plan, then applies it after confirmation.

Usage: temper [GLOBAL OPTIONS] apply [OPTIONS]

Options:
  --yes                   Apply the provisioning plan without confirmation
  --existing-repo         Legacy compatibility: require every configured repo
                           to already exist and do not seed repository content
  -h, --help              Print help";

/// How `temper apply` handles local credentials after provisioning.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ApplyCredentialMode {
    /// Do not mutate the selected credentials file after provisioning.
    SkipLocalCredentials,
    /// Init-local compatibility: merge minted tokens into `credentials.toml`
    /// after every repository plan succeeds.
    UpdateLocalCredentials,
}

impl Default for ApplyCredentialMode {
    fn default() -> Self {
        Self::UpdateLocalCredentials
    }
}

/// Everything `temper apply` needs beyond the loaded deployment.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Where to read `config.toml` and read/write `credentials.toml`.
    pub options: LoadOptions,
    /// Provision onto repos that must already exist (`--existing-repo`).
    pub existing_repo: bool,
    /// Skip the confirmation before forge-side mutations.
    pub yes: bool,
    /// Whether to merge minted tokens into the local credentials file after a
    /// successful apply. This is deliberately explicit so tests and future
    /// production callers can distinguish provisioning from local secret-file
    /// mutation.
    pub credential_mode: ApplyCredentialMode,
    /// Environment snapshot used for path expansion and for systemd
    /// `CREDENTIALS_DIRECTORY` credentials discovery.
    pub env: EnvMap,
    /// Base directories used to resolve default config locations.
    pub paths: PathResolver,
}

#[derive(Debug, Clone, Default)]
struct ParsedApplyArgs {
    help: bool,
    options: LoadOptions,
    existing_repo: bool,
    yes: bool,
}

/// The unified binary's `temper apply` entry point.
pub fn apply_main(args: Vec<String>, env: &EnvMap, paths: &PathResolver) -> ExitCode {
    apply_main_with_options(args, env, paths, LoadOptions::default())
}

pub fn apply_main_with_options(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let parsed = match parse_apply_args(args, options) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper apply: {error}\n\n{APPLY_USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{APPLY_USAGE}");
        return ExitCode::SUCCESS;
    }

    let opts = ApplyOptions {
        options: parsed.options,
        existing_repo: parsed.existing_repo,
        yes: parsed.yes,
        credential_mode: ApplyCredentialMode::UpdateLocalCredentials,
        env: env.clone(),
        paths: paths.clone(),
    };
    let mut prompter = TerminalPrompter::stdio();
    let mut provisioner = ForgejoProvisioner;
    match run_apply(&mut prompter, &mut provisioner, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper apply: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_apply_args(args: Vec<String>, options: LoadOptions) -> Result<ParsedApplyArgs, String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedApplyArgs {
            help: true,
            options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedApplyArgs {
        options,
        ..Default::default()
    };
    for arg in args {
        match arg.as_str() {
            "--existing-repo" => parsed.existing_repo = true,
            "--yes" => parsed.yes = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}

/// Loads a deployment, shows its provisioning plan, applies it, and optionally
/// updates local credentials after every repository succeeds.
pub fn run_apply(
    p: &mut dyn Prompter,
    provisioner: &mut dyn ApplyProvisioner,
    opts: &ApplyOptions,
) -> Result<(), InitError> {
    let bundle = load_apply_bundle(opts)?;
    show_apply_plan(p, &bundle);

    if !opts.yes
        && !p.confirm(
            &format!(
                "Apply this provisioning plan to {} repo(s) on {}?",
                bundle.summary.repos.len(),
                bundle.request.base_url,
            ),
            false,
        )?
    {
        p.note("Skipped forge provisioning at operator confirmation.");
        return Ok(());
    }

    let outcome = provisioner
        .provision_apply_plan(&bundle.request)
        .map_err(InitError::Provision)?;
    if outcome.provisioned.len() != bundle.request.plans.len() {
        return Err(InitError::Provision(format!(
            "provisioner returned {} result(s) for {} repo plan(s)",
            outcome.provisioned.len(),
            bundle.request.plans.len()
        )));
    }

    if matches!(
        bundle.credential_mode,
        ApplyCredentialMode::UpdateLocalCredentials
    ) {
        if let Some(admin_key) = bundle.admin_key.as_deref() {
            let mut credentials = bundle.credentials;
            merge_provisioned_credentials(&mut credentials, admin_key, &outcome);
            temper_config::write_credentials(&credentials, &bundle.credentials_path, true)
                .map_err(|error| InitError::Write(error.to_string()))?;
            p.note(&format!(
                "Updated {} (chmod 600)",
                bundle.credentials_path.display()
            ));
        } else {
            p.note(
                "Local credentials were not modified because `[forge] admin` is not configured.",
            );
        }
    } else {
        p.note("Local credentials were not modified.");
    }

    p.note(&format!(
        "Provisioned {} repo(s) on {}.",
        outcome.provisioned.len(),
        bundle.request.base_url,
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

struct ApplyBundle {
    request: ApplyPlanRequest,
    credentials: Credentials,
    credentials_path: PathBuf,
    admin_key: Option<String>,
    credential_mode: ApplyCredentialMode,
    summary: ApplyPlanSummary,
}

struct ApplyPlanSummary {
    deployment: Option<String>,
    topology: Option<String>,
    workflow_source: String,
    repos: Vec<ApplyRepoSummary>,
    worker_pools: usize,
    agent_profiles: usize,
}

struct ApplyRepoSummary {
    owner: String,
    name: String,
    default_branch: String,
    roles: usize,
    labels: usize,
    webhook: bool,
    existing_repo: bool,
}

fn load_apply_bundle(opts: &ApplyOptions) -> Result<ApplyBundle, InitError> {
    let targets =
        resolve_targets(&opts.options, &opts.env, &opts.paths).map_err(InitError::Path)?;
    let config = Config::load(&targets.config)
        .map_err(|error| InitError::Path(format!("load {}: {error}", targets.config.display())))?;
    let credentials = Credentials::load(&targets.credentials).map_err(|error| {
        InitError::Path(format!("load {}: {error}", targets.credentials.display()))
    })?;
    let config_base = targets
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let resolved = temper_config::resolve_with_options(
        &config,
        &credentials,
        &opts.env,
        &ResolveOptions::from_config_base_dir(config_base),
    )
    .map_err(|error| InitError::Path(format!("resolve deployment: {error}")))?;

    let base_url = resolved.forge.url.clone().ok_or_else(|| {
        InitError::Unsupported("temper apply requires `[forge] url` in config.toml".to_string())
    })?;
    if resolved.engine.repos.is_empty() {
        return Err(InitError::Unsupported(
            "temper apply requires at least one `[engine] repos` entry".to_string(),
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
    let admin_password = admin.and_then(|user| non_empty(user.password.as_deref()));
    let admin_token = resolved
        .forge
        .admin_token
        .as_ref()
        .map(|token| token.expose_secret().to_string());
    if admin_token.is_none() && (admin_user.is_none() || admin_password.is_none()) {
        return Err(InitError::Unsupported(
            "temper apply requires an admin forge token (for example `[engine] forge_token`) \
             or a legacy `[forge] admin` user with a password in credentials.toml"
                .to_string(),
        ));
    }

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
        Some(temper_forge::WebhookSpec {
            url: webhook_url,
            secret: secret.expose_secret().to_string(),
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

    let mut plans = Vec::with_capacity(resolved.engine.repos.len());
    let mut repo_summaries = Vec::with_capacity(resolved.engine.repos.len());
    for repo in &resolved.engine.repos {
        let plan = build_deployment_repo_plan(
            &workflow,
            &repo.owner,
            &repo.name,
            webhook.clone(),
            opts.existing_repo,
        )
        .map_err(InitError::Unsupported)?;
        repo_summaries.push(ApplyRepoSummary {
            owner: plan.repo.owner.clone(),
            name: plan.repo.name.clone(),
            default_branch: plan.default_branch.clone(),
            roles: plan.roles.len(),
            labels: plan.labels.len(),
            webhook: plan.webhook.is_some(),
            existing_repo: plan.existing_repo,
        });
        plans.push(plan);
    }

    let request = ApplyPlanRequest {
        base_url,
        admin_user,
        admin_password,
        admin_token,
        plans,
    };
    let summary = ApplyPlanSummary {
        deployment: resolved.deployment.name.clone(),
        topology: resolved
            .deployment
            .topology
            .map(|topology| topology.as_str().to_string()),
        workflow_source,
        repos: repo_summaries,
        worker_pools: resolved.worker.pools.len(),
        agent_profiles: resolved.agent.profiles.len(),
    };

    Ok(ApplyBundle {
        request,
        credentials,
        credentials_path: targets.credentials,
        admin_key,
        credential_mode: opts.credential_mode,
        summary,
    })
}

fn load_webhook(path: &Path, webhook_url: &str) -> Result<temper_forge::WebhookSpec, InitError> {
    let secret = std::fs::read_to_string(path)
        .map_err(|error| {
            InitError::Path(format!("read webhook secret {}: {error}", path.display()))
        })?
        .trim()
        .to_string();
    Ok(temper_forge::WebhookSpec {
        url: webhook_url.to_string(),
        secret,
        events: temper_forge::WebhookEvents::All,
    })
}

fn show_apply_plan(p: &mut dyn Prompter, bundle: &ApplyBundle) {
    p.note("Apply plan:");
    p.note(&format!("  forge: {}", bundle.request.base_url));
    match (&bundle.summary.deployment, &bundle.summary.topology) {
        (Some(name), Some(topology)) => p.note(&format!("  deployment: {name} ({topology})")),
        (Some(name), None) => p.note(&format!("  deployment: {name}")),
        (None, Some(topology)) => p.note(&format!("  topology: {topology}")),
        (None, None) => {}
    }
    p.note(&format!("  workflow: {}", bundle.summary.workflow_source));
    p.note(&format!(
        "  repositories: {} repo(s)",
        bundle.summary.repos.len()
    ));
    for repo in &bundle.summary.repos {
        let mode = if repo.existing_repo {
            "require existing repository"
        } else {
            "create if missing"
        };
        let webhook = if repo.webhook { "yes" } else { "no" };
        p.note(&format!(
            "  - {}/{}: {mode}, branch {}, {} role(s), {} label(s), webhook {webhook}",
            repo.owner, repo.name, repo.default_branch, repo.roles, repo.labels,
        ));
    }
    if bundle.summary.repos.iter().any(|repo| repo.existing_repo) {
        p.note("  --existing-repo compatibility: applies to every configured repository");
    }
    if matches!(
        bundle.credential_mode,
        ApplyCredentialMode::UpdateLocalCredentials
    ) {
        if bundle.admin_key.is_some() {
            p.note(&format!(
                "  credentials: update {} after success",
                bundle.credentials_path.display()
            ));
        } else {
            p.note("  credentials: not modified (`[forge] admin` is not configured)");
        }
    } else {
        p.note("  credentials: not modified");
    }
    if bundle.summary.worker_pools > 0 || bundle.summary.agent_profiles > 0 {
        p.note(&format!(
            "  metadata: {} worker pool(s), {} agent profile(s)",
            bundle.summary.worker_pools, bundle.summary.agent_profiles,
        ));
    }
}

fn merge_provisioned_credentials(
    credentials: &mut Credentials,
    admin_key: &str,
    outcome: &ApplyPlanOutcome,
) {
    for provisioned in &outcome.provisioned {
        let single = ProvisionOutcome {
            provisioned: provisioned.clone(),
            admin_token: outcome.admin_token.clone(),
        };
        for (key, user) in write::provisioned_role_and_bot_users(&single) {
            credentials.forge.users.insert(key, user);
        }
    }
    let admin = credentials
        .forge
        .users
        .entry(admin_key.to_string())
        .or_default();
    admin.token = Some(outcome.admin_token.clone());
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
