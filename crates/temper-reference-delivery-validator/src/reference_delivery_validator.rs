//! Reference-delivery Forge-state validator.
//!
//! This is the operator-facing validation path used by the shell demo. It reads
//! Forge state through the portable [`Forge`] trait and emits bounded diagnostic
//! lines; it never shells out to token-bearing provider APIs.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use temper_forge_model::{
    Forge, ForgeError, Issue, IssueState, ItemNumber, RepositoryId, RepositoryPath,
};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_workflow::{ArtifactRef, WorkflowMetadata, parse_metadata_block};

/// Environment variable carrying a read-capable Forgejo token for validation.
pub const VALIDATOR_TOKEN_ENV: &str = "TEMPER_FORGEJO_TOKEN";

pub const USAGE: &str = concat!(
    "temper-validate-reference-delivery --base-url <url> ",
    "--repo <owner/name> [--repo <owner/name> ...] ",
    "--source-repo <owner/name> --parent-number <n> --expected-children <n>\n",
    "  the read token comes from TEMPER_FORGEJO_TOKEN (required), never argv"
);

/// Configuration for a reference-delivery state validation pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatorConfig {
    /// Repository that contains the cross-repo parent intake.
    pub source_repo: RepositoryPath,
    /// Full configured repository set expected to receive child work.
    pub repositories: Vec<RepositoryPath>,
    /// Source parent issue number.
    pub parent_number: ItemNumber,
    /// Expected number of child dependency links.
    pub expected_children: usize,
}

/// Parsed CLI arguments for the validator binary.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatorArgs {
    pub base_url: String,
    pub token: String,
    pub config: ValidatorConfig,
}

impl fmt::Debug for ValidatorArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatorArgs")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .field("config", &self.config)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Run(ValidatorArgs),
    Help,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ArgsError {}

#[derive(Debug)]
pub enum RunError {
    Runtime(String),
    Backend(ForgeError),
    ValidationFailed(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(message) => write!(formatter, "failed to start async runtime: {message}"),
            Self::Backend(error) => write!(formatter, "forge validation read failed: {error}"),
            Self::ValidationFailed(output) => formatter.write_str(output),
        }
    }
}

impl Error for RunError {}

impl From<ForgeError> for RunError {
    fn from(error: ForgeError) -> Self {
        Self::Backend(error)
    }
}

/// Bounded, human-readable validation output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValidationReport {
    lines: Vec<String>,
    failures: usize,
}

impl ValidationReport {
    fn ok(&mut self, message: impl Into<String>) {
        self.lines.push(format!("ok: {}", message.into()));
    }

    fn missing(&mut self, message: impl Into<String>) {
        self.failures = self.failures.saturating_add(1);
        self.lines.push(format!("missing: {}", message.into()));
    }

    fn diagnosis(&mut self, message: impl Into<String>) {
        self.lines.push(format!("diagnosis: {}", message.into()));
    }

    /// Returns true when no missing invariant was reported.
    pub fn is_ok(&self) -> bool {
        self.failures == 0
    }

    /// Renders the report as newline-separated, bounded diagnostic lines.
    pub fn render(&self) -> String {
        self.lines.join("\n")
    }

    /// Returns the report lines for assertions.
    pub fn lines(&self) -> &[String] {
        &self.lines
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
    let mut repos = Vec::new();
    let mut source_repo = None;
    let mut parent_number = None;
    let mut expected_children = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--base-url" => base_url = Some(value_for(&flag, &mut iter)?),
            "--repo" => repos.push(parse_repo_path(&value_for(&flag, &mut iter)?)?),
            "--source-repo" => source_repo = Some(parse_repo_path(&value_for(&flag, &mut iter)?)?),
            "--parent-number" => {
                parent_number = Some(parse_item_number(&value_for(&flag, &mut iter)?)?)
            }
            "--expected-children" => {
                expected_children = Some(parse_usize(&value_for(&flag, &mut iter)?)?)
            }
            other => {
                return Err(ArgsError(format!(
                    "unrecognized argument '{other}'\nusage: {USAGE}"
                )));
            }
        }
    }
    let token = env(VALIDATOR_TOKEN_ENV)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ArgsError(format!(
                "missing required environment variable {VALIDATOR_TOKEN_ENV}"
            ))
        })?;
    let source_repo = require(source_repo, "--source-repo")?;
    if repos.is_empty() {
        return Err(ArgsError(format!(
            "missing at least one --repo\nusage: {USAGE}"
        )));
    }
    Ok(ParseOutcome::Run(ValidatorArgs {
        base_url: require(base_url, "--base-url")?,
        token,
        config: ValidatorConfig {
            source_repo,
            repositories: repos,
            parent_number: require(parent_number, "--parent-number")?,
            expected_children: require(expected_children, "--expected-children")?,
        },
    }))
}

pub fn run(args: &ValidatorArgs) -> Result<String, RunError> {
    let runtime = temper_engine_io::build_runtime().map_err(RunError::Runtime)?;
    let forge = ForgejoForge::new(ForgejoConfig::new(
        args.base_url.clone(),
        args.token.clone(),
    ));
    let report = runtime.block_on(validate_state(&forge, &args.config))?;
    let rendered = report.render();
    if report.is_ok() {
        Ok(rendered)
    } else {
        Err(RunError::ValidationFailed(rendered))
    }
}

/// Validates the reference-delivery parent/child dependency shape in Forge state.
pub async fn validate_state<F: Forge + ?Sized>(
    forge: &F,
    config: &ValidatorConfig,
) -> Result<ValidationReport, ForgeError> {
    let mut report = ValidationReport::default();
    let repos = resolve_repositories(forge, &config.repositories, &mut report).await?;
    let Some(source_repo) = repo_id_for(&repos, &config.source_repo) else {
        report.missing(format!(
            "source repository {} is not in the resolved repository set",
            display_path(&config.source_repo)
        ));
        return Ok(report);
    };

    let Some(parent) = forge
        .get_issue_by_number(source_repo, config.parent_number)
        .await?
    else {
        report.missing(format!(
            "cross-repo parent {}#{} exists",
            display_path(&config.source_repo),
            config.parent_number
        ));
        return Ok(report);
    };
    report.ok(format!(
        "cross-repo parent {}#{} exists",
        display_path(&config.source_repo),
        config.parent_number
    ));

    let parent_metadata = optional_metadata_for("parent", &parent, &mut report);
    let dependencies = dependency_refs(&parent, parent_metadata.as_ref());
    let parent_blocked = parent.labels.iter().any(|label| label == "blocked");
    validate_parent_dependency_count(
        &mut report,
        &config.source_repo,
        config.parent_number,
        parent_blocked,
        dependencies.len(),
        config.expected_children,
    );
    validate_children(
        forge,
        &mut report,
        source_repo,
        &repos,
        config,
        &dependencies,
        parent_blocked,
    )
    .await?;
    Ok(report)
}

async fn resolve_repositories<F: Forge + ?Sized>(
    forge: &F,
    paths: &[RepositoryPath],
    report: &mut ValidationReport,
) -> Result<BTreeMap<String, RepositoryId>, ForgeError> {
    let mut repos = BTreeMap::new();
    for path in paths {
        match forge.get_repository_by_path(path).await? {
            Some(repo) => {
                report.ok(format!("repository {} is readable", display_path(path)));
                repos.insert(display_path(path), repo.id);
            }
            None => report.missing(format!("repository {} is readable", display_path(path))),
        }
    }
    Ok(repos)
}

fn validate_parent_dependency_count(
    report: &mut ValidationReport,
    source: &RepositoryPath,
    parent: ItemNumber,
    parent_blocked: bool,
    observed: usize,
    expected: usize,
) {
    if parent_blocked && observed == 0 {
        report.missing(format!(
            "blocked parent {}#{} has zero dependencies",
            display_path(source),
            parent
        ));
        report.diagnosis(concat!(
            "dependency-gated unblocking intentionally cannot proceed without at least ",
            "one recorded dependency"
        ));
    }
    if observed == expected {
        report.ok(format!(
            "cross-repo parent {}#{} expected {expected} child dependencies, found {observed}",
            display_path(source),
            parent
        ));
    } else {
        report.missing(format!(
            "cross-repo parent {}#{} expected {expected} child dependencies, found {observed}",
            display_path(source),
            parent
        ));
        if parent_blocked && observed == 0 {
            report.diagnosis(
                "architect blocked the parent but no fan-out side effects were recorded",
            );
        }
    }
}

async fn validate_children<F: Forge + ?Sized>(
    forge: &F,
    report: &mut ValidationReport,
    source_repo: &RepositoryId,
    repos: &BTreeMap<String, RepositoryId>,
    config: &ValidatorConfig,
    dependencies: &[ArtifactRef],
    parent_blocked: bool,
) -> Result<(), ForgeError> {
    let mut landed = 0usize;
    for dependency in dependencies {
        let child_repo = dependency.resolved_repository(source_repo);
        let child_display = display_repo_id(repos, &child_repo);
        let Some(child) = forge
            .get_issue_by_number(&child_repo, dependency.number)
            .await?
        else {
            report.missing(format!(
                "child dependency {child_display}#{} exists",
                dependency.number
            ));
            continue;
        };
        if child.state == IssueState::Closed {
            landed = landed.saturating_add(1);
        }
        validate_child_metadata(
            report,
            source_repo,
            &child_repo,
            config.parent_number,
            &child_display,
            &child,
        );
    }
    if dependencies.is_empty() {
        return Ok(());
    }
    report.ok(format!(
        "child landed count {landed}/{} (closed issues count as landed dependency targets)",
        dependencies.len()
    ));
    if landed == dependencies.len() && parent_blocked {
        report.missing(format!(
            "parent {}#{} remains blocked even though all child dependencies landed",
            display_path(&config.source_repo),
            config.parent_number
        ));
        report.diagnosis(concat!(
            "mechanical reconciliation should clear the dependency gate; inspect ",
            "mechanical_reconciliation events"
        ));
    }
    Ok(())
}

fn validate_child_metadata(
    report: &mut ValidationReport,
    source_repo: &RepositoryId,
    child_repo: &RepositoryId,
    parent_number: ItemNumber,
    child_display: &str,
    child: &Issue,
) {
    let Some(metadata) = required_metadata_for("child", child, report) else {
        return;
    };
    let has_parent = metadata.parents.iter().any(|parent| {
        parent.number == parent_number && parent.resolved_repository(child_repo) == *source_repo
    });
    if has_parent {
        report.ok(format!(
            "child {child_display}#{} carries parent back-reference",
            child.number
        ));
    } else {
        report.missing(format!(
            concat!("child {}#{} carries parent back-reference to ", "{}#{}"),
            child_display, child.number, source_repo, parent_number
        ));
    }
    if metadata
        .correlation_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
    {
        report.ok(format!(
            "child {child_display}#{} carries correlation metadata",
            child.number
        ));
    } else {
        report.missing(format!(
            "child {child_display}#{} carries correlation metadata",
            child.number
        ));
    }
}

fn optional_metadata_for(
    label: &str,
    issue: &Issue,
    report: &mut ValidationReport,
) -> Option<WorkflowMetadata> {
    match parse_metadata_block(&issue.body) {
        Ok(metadata) => metadata,
        Err(error) => {
            report.missing(format!(
                "{label} issue #{} has parseable workflow metadata: {error}",
                issue.number
            ));
            None
        }
    }
}

fn required_metadata_for(
    label: &str,
    issue: &Issue,
    report: &mut ValidationReport,
) -> Option<WorkflowMetadata> {
    let metadata = optional_metadata_for(label, issue, report);
    if metadata.is_none() {
        report.missing(format!(
            "{label} issue #{} carries workflow metadata",
            issue.number
        ));
    }
    metadata
}

fn dependency_refs(parent: &Issue, metadata: Option<&WorkflowMetadata>) -> Vec<ArtifactRef> {
    let mut refs = BTreeSet::new();
    refs.extend(
        parent
            .dependencies
            .iter()
            .copied()
            .map(ArtifactRef::same_repo),
    );
    if let Some(metadata) = metadata {
        refs.extend(metadata.dependencies.iter().cloned());
    }
    refs.into_iter().collect()
}

fn repo_id_for<'a>(
    repos: &'a BTreeMap<String, RepositoryId>,
    path: &RepositoryPath,
) -> Option<&'a RepositoryId> {
    repos.get(&display_path(path))
}

fn display_repo_id(repos: &BTreeMap<String, RepositoryId>, repo_id: &RepositoryId) -> String {
    repos
        .iter()
        .find_map(|(display, candidate)| (candidate == repo_id).then(|| display.clone()))
        .unwrap_or_else(|| repo_id.to_string())
}

fn parse_repo_path(value: &str) -> Result<RepositoryPath, ArgsError> {
    let Some((owner, name)) = value.split_once('/') else {
        return Err(ArgsError(format!(
            "repository must be owner/name, got '{value}'"
        )));
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(ArgsError(format!(
            "repository must be owner/name with non-empty parts, got '{value}'"
        )));
    }
    Ok(RepositoryPath::new(owner, name))
}

fn display_path(path: &RepositoryPath) -> String {
    format!("{}/{}", path.owner, path.name)
}

fn parse_item_number(value: &str) -> Result<ItemNumber, ArgsError> {
    let number = value
        .parse::<u64>()
        .map_err(|_| ArgsError(format!("expected positive integer, got '{value}'")))?;
    if number == 0 {
        return Err(ArgsError("item numbers start at 1".to_string()));
    }
    Ok(ItemNumber::new(number))
}

fn parse_usize(value: &str) -> Result<usize, ArgsError> {
    value
        .parse::<usize>()
        .map_err(|_| ArgsError(format!("expected non-negative integer, got '{value}'")))
}

fn value_for<I>(flag: &str, iter: &mut I) -> Result<String, ArgsError>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| ArgsError(format!("flag '{flag}' expects a value")))
}

fn require<T>(value: Option<T>, flag: &str) -> Result<T, ArgsError> {
    value.ok_or_else(|| ArgsError(format!("missing required {flag}\nusage: {USAGE}")))
}

#[cfg(test)]
#[path = "reference_delivery_validator_tests.rs"]
mod reference_delivery_validator_tests;
