// SPDX-License-Identifier: MPL-2.0

//! `temper plan` — deployment-wide provisioning/reconciliation preview.
//!
//! This module owns the shared, serializable planning model for first-run
//! deployment reconciliation. The model is distilled from the same
//! [`ProvisionPlan`](temper_provision::ProvisionPlan) that `temper apply` uses,
//! then enriched with read-only Forge state so operators can see what would be
//! created or updated before any mutating apply pass runs.

mod report;

use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;

use temper_cli_common::{
    EX_USAGE, EnvMap, LoadOptions, OutputFormat, PathResolver, resolve_targets,
};
use temper_config::{Config, Credentials, ExposeSecret, LoadedPaths, ResolveOptions, Resolved};
use temper_forge::{
    Forge, IssueQuery, ItemListDetails, PullRequestQuery, Repository, RepositoryPath,
    WebhookEvents, WebhookStatus,
};
use temper_provision::ProvisionPlan;
use temper_workflow::{ValidatedWorkflow, parse_metadata_block};

use crate::InitError;
use crate::provisioner::{ProvisionRequest, build_init_plan};

pub use report::DeploymentPlanReport;
use report::{build_report, print_report};

/// `temper plan [OPTIONS]` usage.
pub const PLAN_USAGE: &str = "\
Preview a temper deployment bundle without mutating the forge.

Loads config.toml + credentials.toml + workflow, validates them, builds the same
forge provisioning model that `temper apply` uses, then inspects current forge
state with read-only calls. Secret values are never printed.

Usage: temper [GLOBAL OPTIONS] plan [OPTIONS]

Options:
  --existing-repo         Plan onto a repo that must already exist
  -h, --help              Print help

Global options:
  -c, --config <DIR|FILE>      Path to configuration file or bundle directory
      --secrets <DIR|FILE>     Explicit credentials.toml
      --format <human|json>    Output format";

#[derive(Debug, Clone, Default)]
struct ParsedPlanArgs {
    help: bool,
    options: LoadOptions,
    existing_repo: bool,
}

/// Everything `temper plan` needs beyond the loaded bundle.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Where to read `config.toml` and `credentials.toml`.
    pub options: LoadOptions,
    /// Match `temper apply --existing-repo`.
    pub existing_repo: bool,
    /// Requested output format.
    pub format: OutputFormat,
    /// Environment snapshot used for path expansion.
    pub env: EnvMap,
    /// Base directories used to resolve default config locations.
    pub paths: PathResolver,
}

/// The unified binary's `temper plan` entry point.
pub fn plan_main_with_options(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    options: LoadOptions,
    format: OutputFormat,
) -> ExitCode {
    let parsed = match parse_plan_args(args, options) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper plan: {error}\n\n{PLAN_USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{PLAN_USAGE}");
        return ExitCode::SUCCESS;
    }

    let opts = PlanOptions {
        options: parsed.options,
        existing_repo: parsed.existing_repo,
        format,
        env: env.clone(),
        paths: paths.clone(),
    };
    match run_plan(&opts) {
        Ok(report) => {
            if let Err(error) = print_report(&report, format) {
                eprintln!("temper plan: {error}");
                return ExitCode::FAILURE;
            }
            if report.has_error_findings() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("temper plan: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_plan_args(args: Vec<String>, options: LoadOptions) -> Result<ParsedPlanArgs, String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedPlanArgs {
            help: true,
            options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedPlanArgs {
        options,
        ..Default::default()
    };
    for arg in args {
        match arg.as_str() {
            "--existing-repo" => parsed.existing_repo = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}

/// Builds a deployment plan report using the production read-only Forge adapter.
pub fn run_plan(opts: &PlanOptions) -> Result<DeploymentPlanReport, String> {
    let bundle = load_plan_bundle(opts).map_err(|error| error.to_string())?;
    let mut inspector = ForgePlanInspector::from_bundle(&bundle);
    build_report(&bundle, &mut inspector)
}

/// Test seam for no-mutation and adapter-fake coverage.
pub trait DeploymentInspector {
    /// Reads forge state and returns a snapshot. Implementations must not mutate
    /// the backend.
    fn inspect(&mut self, bundle: &PlanBundle) -> Result<ForgeInspection, String>;
}

/// Loaded, validated deployment inputs plus the shared provisioning model.
pub struct PlanBundle {
    pub loaded: LoadedPaths,
    pub resolved: Resolved,
    pub credentials: Credentials,
    pub admin_key: String,
    pub admin_user: String,
    pub request: ProvisionRequest,
    pub workflow: ValidatedWorkflow,
    pub provision_plan: ProvisionPlan,
}

pub(crate) fn load_plan_bundle(opts: &PlanOptions) -> Result<PlanBundle, InitError> {
    let targets =
        resolve_targets(&opts.options, &opts.env, &opts.paths).map_err(InitError::Path)?;
    let config = Config::load(&targets.config)
        .map_err(|error| InitError::Path(format!("load {}: {error}", targets.config.display())))?;
    let credentials = Credentials::load(&targets.credentials).map_err(|error| {
        InitError::Path(format!("load {}: {error}", targets.credentials.display()))
    })?;
    let mut resolve_options = targets
        .config
        .parent()
        .map(ResolveOptions::from_config_base_dir)
        .unwrap_or_default();
    // Generated init bundles reference the forge token that the first apply
    // pass mints. Planning/apply must load that pre-apply bundle, while normal
    // runtime/check paths keep strict secret-reference validation.
    resolve_options.validate_secret_references = false;
    let resolved =
        temper_config::resolve_with_options(&config, &credentials, &opts.env, &resolve_options)
            .map_err(|error| InitError::Path(format!("resolve deployment: {error}")))?;

    let base_url = resolved.forge.url.clone().ok_or_else(|| {
        InitError::Unsupported("deployment plan requires `[forge] url` in config.toml".to_string())
    })?;
    if resolved.engine.repos.len() != 1 {
        return Err(InitError::Unsupported(format!(
            "deployment plan requires exactly one `[engine] repos` entry, found {}",
            resolved.engine.repos.len()
        )));
    }
    let repo = &resolved.engine.repos[0];
    let webhook_secret_file = resolved.engine.webhook_secret_file.clone().ok_or_else(|| {
        InitError::Unsupported(
            "deployment plan requires `[engine] webhook_secret_file` in config.toml".to_string(),
        )
    })?;

    let admin_key = non_empty(config.forge.admin.as_deref()).ok_or_else(|| {
        InitError::Unsupported(
            "deployment plan requires `[forge] admin` in config.toml".to_string(),
        )
    })?;
    let admin = credentials.forge.users.get(&admin_key).ok_or_else(|| {
        InitError::Unsupported(format!(
            "deployment plan requires `[forge.users.{admin_key}]` in credentials.toml"
        ))
    })?;
    let admin_user = non_empty(admin.user.as_deref()).unwrap_or_else(|| admin_key.clone());
    let admin_password = non_empty(admin.password.as_deref()).ok_or_else(|| {
        InitError::Unsupported(format!(
            "deployment plan requires a password under `[forge.users.{admin_key}]` in credentials.toml"
        ))
    })?;

    let request = ProvisionRequest {
        base_url,
        admin_user: admin_user.clone(),
        admin_password,
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        webhook_url: format!("http://{}/forgejo/webhook", resolved.engine.bind),
        webhook_secret_file,
        workflow_path: resolved.engine.workflow_file.clone(),
        existing_repo: opts.existing_repo,
    };
    let workflow = match &request.workflow_path {
        Some(path) => temper_reference_delivery::load_workflow(path)
            .map_err(|error| InitError::Unsupported(error.to_string()))?,
        None => temper_reference_delivery::basic_delivery_workflow(),
    };
    let webhook = temper_forge::WebhookSpec {
        url: request.webhook_url.clone(),
        secret: read_webhook_secret(&request.webhook_secret_file)?,
        events: WebhookEvents::All,
    };
    let provision_plan = build_init_plan(&request, webhook).map_err(InitError::Unsupported)?;

    Ok(PlanBundle {
        loaded: LoadedPaths {
            config: Some(targets.config),
            credentials: Some(targets.credentials),
        },
        resolved,
        credentials,
        admin_key,
        admin_user,
        request,
        workflow,
        provision_plan,
    })
}

fn read_webhook_secret(path: &std::path::Path) -> Result<String, InitError> {
    std::fs::read_to_string(path)
        .map_err(|error| {
            InitError::Path(format!("read webhook secret {}: {error}", path.display()))
        })
        .map(|secret| secret.trim().to_string())
}

pub(super) fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Read-only snapshot returned by a deployment inspector.
#[derive(Clone, Debug, Default)]
pub struct ForgeInspection {
    pub inspected: bool,
    pub unavailable_reason: Option<String>,
    pub repository: Option<Repository>,
    pub labels: Vec<String>,
    pub webhooks: Vec<WebhookStatus>,
    pub ci_enabled: Option<bool>,
    pub users: BTreeMap<String, bool>,
    pub metadata: MetadataInspection,
}

#[derive(Clone, Debug, Default)]
pub struct MetadataInspection {
    pub checked_artifacts: usize,
    pub invalid: Vec<String>,
}

struct ForgePlanInspector {
    forge: Option<std::sync::Arc<dyn temper_forge::ProvisioningForge>>,
    unavailable_reason: Option<String>,
}

impl ForgePlanInspector {
    fn from_bundle(bundle: &PlanBundle) -> Self {
        let Some(token) = &bundle.resolved.forge.admin_token else {
            return Self {
                forge: None,
                unavailable_reason: Some(
                    "admin token is missing; read-only forge inspection was skipped".to_string(),
                ),
            };
        };
        let config = temper_forge::config::ForgejoConfig::new(
            &bundle.request.base_url,
            token.expose_secret(),
        )
        .with_default_repo(&bundle.request.owner, &bundle.request.name);
        Self {
            forge: Some(temper_forge::factory::new_forgejo_provisioning(config)),
            unavailable_reason: None,
        }
    }
}

impl DeploymentInspector for ForgePlanInspector {
    fn inspect(&mut self, bundle: &PlanBundle) -> Result<ForgeInspection, String> {
        let Some(forge) = self.forge.clone() else {
            return Ok(ForgeInspection {
                inspected: false,
                unavailable_reason: self.unavailable_reason.clone(),
                ..ForgeInspection::default()
            });
        };
        let request = bundle.request.clone();
        let desired_users = desired_users(bundle);
        let declared_kinds: BTreeSet<String> = bundle
            .workflow
            .artifact_kinds()
            .iter()
            .map(|kind| kind.id.as_str().to_string())
            .collect();
        let runtime = temper_engine_io::build_runtime()?;
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            inspect_forge(forge.as_ref(), &request, &desired_users, &declared_kinds)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

async fn inspect_forge(
    forge: &dyn temper_forge::ProvisioningForge,
    request: &ProvisionRequest,
    desired_users: &[String],
    declared_kinds: &BTreeSet<String>,
) -> temper_forge::ForgeResult<ForgeInspection> {
    let path = RepositoryPath::new(&request.owner, &request.name);
    let repository = forge.as_forge().get_repository_by_path(&path).await?;
    let Some(repository) = repository else {
        let mut users = BTreeMap::new();
        for login in desired_users {
            users.insert(
                login.clone(),
                forge
                    .as_readiness()
                    .get_provisioned_user(login)
                    .await?
                    .is_some(),
            );
        }
        return Ok(ForgeInspection {
            inspected: true,
            users,
            ..ForgeInspection::default()
        });
    };

    let labels = forge
        .as_forge()
        .list_labels(&repository.id)
        .await?
        .into_iter()
        .map(|label| label.name)
        .collect();
    let webhooks = forge
        .as_readiness()
        .list_webhook_statuses(&repository.id)
        .await?;
    let ci_enabled = forge
        .as_readiness()
        .repository_ci_enabled(&repository.id)
        .await?;
    let mut users = BTreeMap::new();
    for login in desired_users {
        users.insert(
            login.clone(),
            forge
                .as_readiness()
                .get_provisioned_user(login)
                .await?
                .is_some(),
        );
    }
    let metadata = inspect_metadata(forge.as_forge(), &repository, declared_kinds).await?;

    Ok(ForgeInspection {
        inspected: true,
        repository: Some(repository),
        labels,
        webhooks,
        ci_enabled,
        users,
        metadata,
        unavailable_reason: None,
    })
}

async fn inspect_metadata(
    forge: &dyn Forge,
    repository: &Repository,
    declared_kinds: &BTreeSet<String>,
) -> temper_forge::ForgeResult<MetadataInspection> {
    let mut inspection = MetadataInspection::default();
    let issue_query = IssueQuery {
        details: ItemListDetails::summary(),
        ..IssueQuery::default()
    };
    for issue in forge.list_issues(&repository.id, issue_query).await? {
        inspection.checked_artifacts += 1;
        inspect_body_metadata(
            &mut inspection,
            &format!("issue #{}", issue.number.get()),
            &issue.body,
            declared_kinds,
        );
    }
    let pull_query = PullRequestQuery {
        details: ItemListDetails::summary(),
        ..PullRequestQuery::default()
    };
    for pull in forge.list_pull_requests(&repository.id, pull_query).await? {
        inspection.checked_artifacts += 1;
        inspect_body_metadata(
            &mut inspection,
            &format!("pull request #{}", pull.number.get()),
            &pull.body,
            declared_kinds,
        );
    }
    Ok(inspection)
}

fn inspect_body_metadata(
    inspection: &mut MetadataInspection,
    label: &str,
    body: &str,
    declared_kinds: &BTreeSet<String>,
) {
    match parse_metadata_block(body) {
        Ok(Some(metadata)) => {
            if let Some(kind) = metadata.kind {
                if !declared_kinds.contains(kind.as_str()) {
                    inspection.invalid.push(format!(
                        "{label}: metadata names undeclared artifact kind `{kind}`"
                    ));
                }
            }
        }
        Ok(None) => {}
        Err(error) => inspection
            .invalid
            .push(format!("{label}: malformed workflow metadata: {error}")),
    }
}

pub(super) fn desired_users(bundle: &PlanBundle) -> Vec<String> {
    let mut users = BTreeSet::new();
    users.insert(bundle.admin_user.clone());
    users.insert(bundle.provision_plan.automation_login.clone());
    for binding in &bundle.provision_plan.roles {
        users.insert(binding.user.handle.clone());
    }
    users.into_iter().collect()
}

#[cfg(test)]
mod tests;
