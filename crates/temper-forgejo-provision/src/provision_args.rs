//! Argument parsing and top-level runner for `temper-provision-forgejo`.

use std::fmt;
use std::path::PathBuf;

use temper_workflow::{IntakeAuthor, RoleId};

use crate::provision::{self, AccessScope, ProvisionError, ProvisionOptions};

pub const ADMIN_TOKEN_ENV: &str = "TEMPER_FORGEJO_ADMIN_TOKEN";
pub const WORKFLOW_FILE_ENV: &str = "TEMPER_WORKFLOW_FILE";

pub const USAGE: &str = concat!(
    "temper-provision-forgejo --base-url <url> --owner <org> --name <repo> --out <path> ",
    "[--workflow <path>] ",
    "[--webhook-url <url> --webhook-secret-file <path>] ",
    "[--seed-intake yes|no] [--seed-only] [--intake-title <title>] [--intake-body-file <path>] ",
    "[--existing-repo] [--access org-owners|repo-collaborator]\n",
    "  --seed-only files just the intake issue (no org/users/repo/labels/CI/webhook work), ",
    "reusing the role tokens already written to --out; pair it with a first ",
    "--seed-intake no pass so the entry issue can be filed after the workers are up\n",
    "  --existing-repo provisions onto a repo that must already exist: it never ",
    "creates the repo and never commits CI (the marker workflow or the sentinel), ",
    "so the repo's own .forgejo/workflows/ci.yml and history stay untouched; it ",
    "still ensures labels, the webhook, and Actions enablement\n",
    "  --access selects how identities are granted access (default org-owners, ",
    "today's behavior): org-owners adds every role user and the bot to the org ",
    "Owners team; repo-collaborator instead grants each a repo-scoped write ",
    "collaborator permission and never touches the Owners team\n",
    "  the admin token comes from TEMPER_FORGEJO_ADMIN_TOKEN (required), never argv; ",
    "the workflow file may also come from TEMPER_WORKFLOW_FILE, defaulting to the ",
    "bundled reference-delivery workflow when unset"
);

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ParseOutcome {
    Run(ProvisionArgs),
    Help,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProvisionArgs {
    pub base_url: String,
    pub owner: String,
    pub name: String,
    pub out: PathBuf,
    pub admin_token: String,
    pub webhook_url: Option<String>,
    pub webhook_secret_file: Option<PathBuf>,
    pub seed_intake: bool,
    /// File only the intake issue, skipping all org/users/repo/labels/CI/webhook
    /// provisioning. The authoring role's token is recovered from `out`.
    pub seed_only: bool,
    pub intake_title: Option<String>,
    pub intake_body_file: Option<PathBuf>,
    /// Workflow document to provision against. `None` uses the bundled
    /// reference-delivery workflow, reproducing today's default behavior.
    pub workflow_file: Option<PathBuf>,
    /// Provision onto a repo that must already exist: skip repo creation and the
    /// CI/sentinel commits. Defaults to `false` (throwaway behavior).
    pub existing_repo: bool,
    /// How role users and the `bot` are granted access. Defaults to
    /// [`AccessScope::OrgOwners`] (today's behavior).
    pub access: AccessScope,
}

impl fmt::Debug for ProvisionArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionArgs")
            .field("base_url", &self.base_url)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .field("out", &self.out)
            .field("admin_token", &"<redacted>")
            .field("webhook_url", &self.webhook_url)
            .field("webhook_secret_file", &self.webhook_secret_file)
            .field("seed_intake", &self.seed_intake)
            .field("seed_only", &self.seed_only)
            .field("intake_title", &self.intake_title)
            .field("intake_body_file", &self.intake_body_file)
            .field("workflow_file", &self.workflow_file)
            .field("existing_repo", &self.existing_repo)
            .field("access", &self.access)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl ArgsError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgsError {}

#[derive(Debug)]
pub enum RunError {
    Runtime(String),
    Workflow(temper_reference_delivery::WorkflowLoadError),
    Provision(ProvisionError),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Runtime(why) => write!(formatter, "failed to start async runtime: {why}"),
            RunError::Workflow(error) => write!(formatter, "{error}"),
            RunError::Provision(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<ProvisionError> for RunError {
    fn from(error: ProvisionError) -> Self {
        Self::Provision(error)
    }
}

impl From<temper_reference_delivery::WorkflowLoadError> for RunError {
    fn from(error: temper_reference_delivery::WorkflowLoadError) -> Self {
        Self::Workflow(error)
    }
}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::Provision(ProvisionError::Io(error))
    }
}

pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    parse_with_env(args, |key| std::env::var(key).ok())
}

pub fn parse_with_env<I, E>(args: I, env: E) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
    E: Fn(&str) -> Option<String>,
{
    let mut base_url = None;
    let mut owner = None;
    let mut name = None;
    let mut out = None;
    let mut webhook_url = None;
    let mut webhook_secret_file = None;
    let mut seed_intake = true;
    let mut seed_only = false;
    let mut intake_title = None;
    let mut intake_body_file = None;
    let mut workflow_file = None;
    let mut existing_repo = false;
    let mut access = AccessScope::default();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--base-url" => base_url = Some(value_for(&flag, &mut iter)?),
            "--owner" => owner = Some(value_for(&flag, &mut iter)?),
            "--name" => name = Some(value_for(&flag, &mut iter)?),
            "--out" => out = Some(value_for(&flag, &mut iter)?),
            "--workflow" => workflow_file = Some(value_for(&flag, &mut iter)?),
            "--webhook-url" => webhook_url = Some(value_for(&flag, &mut iter)?),
            "--webhook-secret-file" => webhook_secret_file = Some(value_for(&flag, &mut iter)?),
            "--seed-intake" => seed_intake = parse_bool(&value_for(&flag, &mut iter)?)?,
            "--seed-only" => seed_only = true,
            "--existing-repo" => existing_repo = true,
            "--access" => access = parse_access(&value_for(&flag, &mut iter)?)?,
            "--intake-title" => intake_title = Some(value_for(&flag, &mut iter)?),
            "--intake-body-file" => intake_body_file = Some(value_for(&flag, &mut iter)?),
            other => {
                return Err(ArgsError::new(format!(
                    "unrecognized argument '{other}'\nusage: {USAGE}"
                )));
            }
        }
    }
    let admin_token = env(ADMIN_TOKEN_ENV)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ArgsError::new(format!(
                "missing required environment variable {ADMIN_TOKEN_ENV}"
            ))
        })?;
    if seed_only && !seed_intake {
        return Err(ArgsError::new(
            "--seed-only files the intake issue and so cannot be combined with --seed-intake no",
        ));
    }
    Ok(ParseOutcome::Run(ProvisionArgs {
        base_url: require(base_url, "--base-url")?,
        owner: require(owner, "--owner")?,
        name: require(name, "--name")?,
        out: PathBuf::from(require(out, "--out")?),
        admin_token,
        webhook_url,
        webhook_secret_file: webhook_secret_file.map(PathBuf::from),
        seed_intake,
        seed_only,
        intake_title,
        intake_body_file: intake_body_file.map(PathBuf::from),
        workflow_file: non_empty(workflow_file)
            .or_else(|| non_empty(env(WORKFLOW_FILE_ENV)))
            .map(PathBuf::from),
        existing_repo,
        access,
    }))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then_some(value))
}

fn build_runtime() -> Result<temper_engine_io::EngineRuntime, RunError> {
    temper_engine_io::build_runtime().map_err(RunError::Runtime)
}

fn intake_seed_from_args(args: &ProvisionArgs) -> Result<provision::IntakeIssueSeed, RunError> {
    Ok(provision::IntakeIssueSeed {
        title: args
            .intake_title
            .clone()
            .unwrap_or_else(|| provision::DEFAULT_INTAKE_TITLE.into()),
        body: match &args.intake_body_file {
            Some(path) => std::fs::read_to_string(path)?,
            None => provision::DEFAULT_INTAKE_BODY.into(),
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
            provision::seed_intake_issue(
                &seed_args.base_url,
                &token,
                &seed_args.owner,
                &seed_args.name,
                &seed,
                &workflow,
            )
            .await
        },
    )?;
    Ok(format!(
        "seeded {}/{}: intake issue #{number}",
        args.owner, args.name,
    ))
}

fn value_for<I>(flag: &str, iter: &mut I) -> Result<String, ArgsError>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| ArgsError::new(format!("flag '{flag}' expects a value")))
}

fn require(value: Option<String>, flag: &str) -> Result<String, ArgsError> {
    value.ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

fn parse_bool(value: &str) -> Result<bool, ArgsError> {
    match value {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        other => Err(ArgsError::new(format!(
            "--seed-intake expects yes|no, got '{other}'"
        ))),
    }
}

fn parse_access(value: &str) -> Result<AccessScope, ArgsError> {
    match value {
        "org-owners" => Ok(AccessScope::OrgOwners),
        "repo-collaborator" => Ok(AccessScope::RepoCollaborator),
        other => Err(ArgsError::new(format!(
            "--access expects org-owners|repo-collaborator, got '{other}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requires_admin_token_from_env() {
        let error = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
            ]
            .into_iter()
            .map(String::from),
            |_| None,
        )
        .unwrap_err();
        assert!(error.to_string().contains(ADMIN_TOKEN_ENV));
    }

    #[test]
    fn parse_reads_token_from_env_and_debug_redacts_it() {
        let outcome = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert!(args.seed_intake);
        let rendered = format!("{args:?}");
        assert!(!rendered.contains("admin-secret"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn parse_allows_disabling_or_customizing_seed_intake() {
        let outcome = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
                "--seed-intake",
                "no",
                "--intake-title",
                "Custom intake",
                "--intake-body-file",
                "body.md",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert!(!args.seed_intake);
        assert_eq!(args.intake_title.as_deref(), Some("Custom intake"));
        assert_eq!(args.intake_body_file, Some(PathBuf::from("body.md")));
    }

    #[test]
    fn parse_seed_only_keeps_seeding_enabled() {
        let outcome = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
                "--seed-only",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert!(args.seed_only);
        assert!(args.seed_intake);
    }

    #[test]
    fn parse_rejects_seed_only_with_seed_intake_no() {
        let error = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
                "--seed-only",
                "--seed-intake",
                "no",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("--seed-only"));
    }

    /// Parses a minimal valid invocation with the given extra args appended,
    /// returning the resulting `ProvisionArgs`. Panics if parsing yields `Help`.
    fn parse_args(extra: &[&str]) -> ProvisionArgs {
        let mut argv = vec![
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
        ];
        argv.extend_from_slice(extra);
        let outcome = parse_with_env(argv.into_iter().map(String::from), |key| {
            (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string())
        })
        .expect("parses");
        match outcome {
            ParseOutcome::Run(args) => args,
            ParseOutcome::Help => panic!("expected run"),
        }
    }

    #[test]
    fn parse_defaults_existing_repo_and_access_to_back_compat() {
        // No flags ⇒ throwaway behavior: create the repo (existing_repo=false)
        // and join the Owners team (AccessScope::OrgOwners).
        let args = parse_args(&[]);
        assert!(!args.existing_repo);
        assert_eq!(args.access, AccessScope::OrgOwners);
    }

    #[test]
    fn parse_existing_repo_flag() {
        let args = parse_args(&["--existing-repo"]);
        assert!(args.existing_repo);
        // `--existing-repo` does not change the access default.
        assert_eq!(args.access, AccessScope::OrgOwners);
    }

    #[test]
    fn parse_access_org_owners_and_repo_collaborator() {
        assert_eq!(
            parse_args(&["--access", "org-owners"]).access,
            AccessScope::OrgOwners
        );
        assert_eq!(
            parse_args(&["--access", "repo-collaborator"]).access,
            AccessScope::RepoCollaborator
        );
    }

    #[test]
    fn parse_rejects_unknown_access_value() {
        let error = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
                "--access",
                "owner",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("--access"), "names the flag: {message}");
        assert!(
            message.contains("org-owners") && message.contains("repo-collaborator"),
            "lists both options: {message}"
        );
    }

    #[test]
    fn parse_existing_repo_does_not_conflict_with_seed_flags() {
        // `--existing-repo` and `--access` compose with the seed knobs; the
        // intended Smith caller pairs `--existing-repo --access repo-collaborator`
        // with `--seed-intake no`.
        let args = parse_args(&[
            "--existing-repo",
            "--access",
            "repo-collaborator",
            "--seed-intake",
            "no",
        ]);
        assert!(args.existing_repo);
        assert_eq!(args.access, AccessScope::RepoCollaborator);
        assert!(!args.seed_intake);
    }

    #[test]
    fn parse_redacts_token_but_shows_new_flags_in_debug() {
        let args = parse_args(&["--existing-repo", "--access", "repo-collaborator"]);
        let rendered = format!("{args:?}");
        assert!(!rendered.contains("admin-secret"));
        assert!(rendered.contains("existing_repo: true"));
        assert!(rendered.contains("RepoCollaborator"));
    }

    #[test]
    fn parse_defaults_workflow_file_to_none() {
        let outcome = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert_eq!(args.workflow_file, None);
    }

    #[test]
    fn parse_accepts_workflow_flag_with_env_fallback_and_precedence() {
        // Flag override.
        let ParseOutcome::Run(args) = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
                "--workflow",
                "from-flag.json",
            ]
            .into_iter()
            .map(String::from),
            |key| match key {
                ADMIN_TOKEN_ENV => Some("admin-secret".to_string()),
                WORKFLOW_FILE_ENV => Some("from-env.json".to_string()),
                _ => None,
            },
        )
        .expect("parses") else {
            panic!("expected run")
        };
        assert_eq!(args.workflow_file, Some(PathBuf::from("from-flag.json")));

        // Env fallback when the flag is absent.
        let ParseOutcome::Run(args) = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
            ]
            .into_iter()
            .map(String::from),
            |key| match key {
                ADMIN_TOKEN_ENV => Some("admin-secret".to_string()),
                WORKFLOW_FILE_ENV => Some("from-env.json".to_string()),
                _ => None,
            },
        )
        .expect("parses") else {
            panic!("expected run")
        };
        assert_eq!(args.workflow_file, Some(PathBuf::from("from-env.json")));
    }
}
