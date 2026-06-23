//! Reference-delivery Forge-state validator.
//!
//! This is the operator-facing validation path used by the shell demo. It reads
//! Forge state through the portable [`Forge`] trait and emits bounded diagnostic
//! lines; it never shells out to token-bearing provider APIs.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use chrono::{DateTime, Utc};
use temper_forge::config::ForgejoConfig;
use temper_forge::{
    Forge, ForgeError, Issue, IssueQuery, IssueState, ItemNumber, PullRequest, PullRequestQuery,
    PullRequestState, RepositoryId, RepositoryPath,
};
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
    let forge = temper_forge::factory::new_forgejo(ForgejoConfig::new(
        args.base_url.clone(),
        args.token.clone(),
    ));
    let report = runtime.block_on(validate_state(forge.as_ref(), &args.config))?;
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
    validate_parent_uniqueness(forge, &mut report, source_repo, &repos, config, &parent).await?;
    validate_child_distribution(&mut report, source_repo, &repos, &dependencies);
    let child_summary = validate_children(
        forge,
        &mut report,
        source_repo,
        &repos,
        config,
        &dependencies,
        parent_blocked,
    )
    .await?;
    validate_parent_resolution(&mut report, config, &parent, &child_summary);
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

async fn validate_parent_uniqueness<F: Forge + ?Sized>(
    forge: &F,
    report: &mut ValidationReport,
    source_repo: &RepositoryId,
    repos: &BTreeMap<String, RepositoryId>,
    config: &ValidatorConfig,
    parent: &Issue,
) -> Result<(), ForgeError> {
    for (display, repo_id) in repos {
        let matches: Vec<Issue> = forge
            .list_issues(repo_id, IssueQuery::default())
            .await?
            .into_iter()
            .filter(|issue| issue.title == parent.title)
            .collect();
        if repo_id == source_repo {
            let source_has_only_parent = matches.len() == 1
                && matches
                    .iter()
                    .any(|issue| issue.number == config.parent_number);
            if source_has_only_parent {
                report.ok(format!(
                    "source repository {display} has one parent intake titled {:?}",
                    parent.title
                ));
            } else {
                report.missing(format!(
                    "source repository {display} has exactly one parent intake titled {:?} (found {})",
                    parent.title,
                    matches.len()
                ));
            }
        } else if matches.is_empty() {
            report.ok(format!(
                "target repository {display} has no duplicate parent intake titled {:?}",
                parent.title
            ));
        } else {
            report.missing(format!(
                "target repository {display} has no duplicate parent intake titled {:?} (found {})",
                parent.title,
                matches.len()
            ));
        }
    }
    Ok(())
}

fn validate_child_distribution(
    report: &mut ValidationReport,
    source_repo: &RepositoryId,
    repos: &BTreeMap<String, RepositoryId>,
    dependencies: &[ArtifactRef],
) {
    if dependencies.is_empty() {
        return;
    }
    let mut counts: BTreeMap<String, usize> = repos.keys().map(|repo| (repo.clone(), 0)).collect();
    for dependency in dependencies {
        let child_repo = dependency.resolved_repository(source_repo);
        if let Some(display) = repos
            .iter()
            .find_map(|(display, repo_id)| (repo_id == &child_repo).then(|| display.clone()))
        {
            if let Some(count) = counts.get_mut(&display) {
                *count = count.saturating_add(1);
            }
        } else {
            report.missing(format!(
                "child dependency {}#{} targets a configured repository",
                child_repo, dependency.number
            ));
        }
    }
    for (display, count) in counts {
        if count == 1 {
            report.ok(format!(
                "repository {display} has exactly one child dependency from the parent"
            ));
        } else {
            report.missing(format!(
                "repository {display} has exactly one child dependency from the parent (found {count})"
            ));
        }
    }
}

struct ChildValidationSummary {
    total: usize,
    landed: usize,
    latest_child_closed: Option<DateTime<Utc>>,
}

async fn validate_children<F: Forge + ?Sized>(
    forge: &F,
    report: &mut ValidationReport,
    source_repo: &RepositoryId,
    repos: &BTreeMap<String, RepositoryId>,
    config: &ValidatorConfig,
    dependencies: &[ArtifactRef],
    parent_blocked: bool,
) -> Result<ChildValidationSummary, ForgeError> {
    let mut landed = 0usize;
    let mut latest_child_closed: Option<DateTime<Utc>> = None;
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
            match child.closed_at {
                Some(closed_at) => {
                    latest_child_closed = Some(
                        latest_child_closed
                            .map(|latest| latest.max(closed_at))
                            .unwrap_or(closed_at),
                    );
                }
                None => report.missing(format!(
                    "closed child dependency {child_display}#{} has a closed_at timestamp",
                    child.number
                )),
            }
        }
        validate_child_metadata(
            report,
            source_repo,
            &child_repo,
            config.parent_number,
            &child_display,
            &child,
        );
        validate_child_merged_pr(forge, report, &child_repo, &child_display, &child).await?;
    }
    if dependencies.is_empty() {
        return Ok(ChildValidationSummary {
            total: 0,
            landed: 0,
            latest_child_closed: None,
        });
    }
    let landed_summary = format!(
        "child landed count {landed}/{} (closed issues count as landed dependency targets)",
        dependencies.len()
    );
    if landed == dependencies.len() {
        report.ok(landed_summary);
    } else {
        report.missing(landed_summary);
        report.diagnosis(
            "wait for each child PR to merge and close its parent code issue via close_parent_issues",
        );
    }
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
    Ok(ChildValidationSummary {
        total: dependencies.len(),
        landed,
        latest_child_closed,
    })
}

fn validate_parent_resolution(
    report: &mut ValidationReport,
    config: &ValidatorConfig,
    parent: &Issue,
    children: &ChildValidationSummary,
) {
    if children.total == 0 || children.landed != children.total {
        return;
    }
    if parent.state == IssueState::Closed {
        report.ok(format!(
            "parent {}#{} is closed after all children landed",
            display_path(&config.source_repo),
            config.parent_number
        ));
    } else {
        report.missing(format!(
            "parent {}#{} is closed after all children landed",
            display_path(&config.source_repo),
            config.parent_number
        ));
        return;
    }
    let Some(latest_child_closed) = children.latest_child_closed else {
        return;
    };
    match parent.closed_at {
        Some(parent_closed) if parent_closed >= latest_child_closed => report.ok(format!(
            "parent {}#{} closed no earlier than the latest child landing",
            display_path(&config.source_repo),
            config.parent_number
        )),
        Some(parent_closed) => report.missing(format!(
            "parent {}#{} closed at {parent_closed} before latest child landing {latest_child_closed}",
            display_path(&config.source_repo),
            config.parent_number
        )),
        None => report.missing(format!(
            "closed parent {}#{} has a closed_at timestamp",
            display_path(&config.source_repo),
            config.parent_number
        )),
    }
}

async fn validate_child_merged_pr<F: Forge + ?Sized>(
    forge: &F,
    report: &mut ValidationReport,
    child_repo: &RepositoryId,
    child_display: &str,
    child: &Issue,
) -> Result<(), ForgeError> {
    let merged = forge
        .list_pull_requests(
            child_repo,
            PullRequestQuery {
                state: Some(PullRequestState::Merged),
                labels: vec!["implementation".to_string()],
                ..PullRequestQuery::default()
            },
        )
        .await?;
    let matches: Vec<PullRequest> = merged
        .into_iter()
        .filter(|pull_request| pull_request_references_child(pull_request, child_repo, child))
        .collect();
    match matches.as_slice() {
        [pull_request] => report.ok(format!(
            "child {child_display}#{} has one merged implementation PR (PR#{})",
            child.number, pull_request.number
        )),
        _ => report.missing(format!(
            "child {child_display}#{} has one merged implementation PR (found {})",
            child.number,
            matches.len()
        )),
    }
    Ok(())
}

fn pull_request_references_child(
    pull_request: &PullRequest,
    child_repo: &RepositoryId,
    child: &Issue,
) -> bool {
    parse_metadata_block(&pull_request.body)
        .ok()
        .flatten()
        .is_some_and(|metadata| {
            metadata.parents.iter().any(|parent| {
                parent.number == child.number
                    && parent.resolved_repository(child_repo) == *child_repo
            })
        })
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
