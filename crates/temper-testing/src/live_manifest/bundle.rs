use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml::Value as TomlValue;

pub use super::execution_plan::{
    AgentFixture, ConvergenceStrategy, ManifestAction, ManifestExecutionPlan, ManifestStep,
};

use super::{
    DEFAULT_CONVERGENCE_SECS, DEFAULT_DAEMON_POLL_BACKSTOP_SECS, DEFAULT_MECHANICAL_CADENCE_SECS,
};

/// Resolved fixtures and typed actions needed by the live manifest topology.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioBundle {
    pub scenario_path: PathBuf,
    pub manifest_path: PathBuf,
    pub workflow_path: PathBuf,
    pub workflow_text: String,
    pub execution: ManifestExecutionPlan,
    pub resolved_manifest: TomlValue,
    pub repo: RepoFixture,
    pub issues: Vec<IssueFixture>,
    pub intake: IntakeFixture,
    pub timeout: Duration,
    pub poll_backstop: Duration,
    pub mechanical_cadence: Duration,
    pub observability: ObservabilityFixture,
}

impl ScenarioBundle {
    /// Loads a live scenario directory (or its manifest file) and the
    /// referenced workflow, repository seed, CI, intake, timeout, and live
    /// backstop settings.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let input = path.as_ref();
        let (scenario_path, manifest_path) = if input.is_file() {
            let manifest_path = fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
            let parent = manifest_path.parent().ok_or_else(|| {
                format!(
                    "manifest {} has no parent directory",
                    manifest_path.display()
                )
            })?;
            (parent.to_path_buf(), manifest_path)
        } else {
            let scenario_path = fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
            let manifest_path = scenario_path.join("scenario.toml");
            (scenario_path, manifest_path)
        };
        let manifest = temper_scenario_core::load_resolved_manifest_toml(&manifest_path)
            .map_err(|error| error.to_string())?;
        Self::from_manifest(scenario_path, manifest_path, manifest)
    }

    /// Builds a live manifest bundle from an already-resolved manifest.
    /// Callers that support fixture inheritance can pass a manifest whose local
    /// path strings have already been rewritten to the files that declared them.
    pub fn from_manifest(
        scenario_path: PathBuf,
        manifest_path: PathBuf,
        manifest: TomlValue,
    ) -> Result<Self, String> {
        let execution = ManifestExecutionPlan::from_manifest(&manifest)?;
        let (workflow_path, workflow_text) = workflow_fixture(&scenario_path, &manifest)?;
        let repo = repo_fixture(&scenario_path, &manifest)?;
        validate_fixture_actions(&execution, &workflow_path, &repo)?;
        let issues = issue_fixtures(&scenario_path, &manifest)?;
        let intake = intake_fixture(&issues)?;
        let timeout = manifest_duration(
            &manifest,
            "timeout",
            Duration::from_secs(DEFAULT_CONVERGENCE_SECS),
        )?;
        let poll_backstop = live_harness_duration(
            &manifest,
            "poll_backstop",
            Duration::from_secs(DEFAULT_DAEMON_POLL_BACKSTOP_SECS),
        )?;
        let mechanical_cadence = live_harness_duration(
            &manifest,
            "mechanical_cadence",
            Duration::from_secs(DEFAULT_MECHANICAL_CADENCE_SECS),
        )?;
        let observability = observability_fixture(&manifest)?;

        Ok(Self {
            scenario_path,
            manifest_path,
            workflow_path,
            workflow_text,
            execution,
            resolved_manifest: manifest,
            repo,
            issues,
            intake,
            timeout,
            poll_backstop,
            mechanical_cadence,
            observability,
        })
    }

    /// Parses and validates the workflow fixture selected by resolved bundle data.
    pub fn validate_workflow(&self) -> Result<(), String> {
        temper_reference_delivery::parse_workflow_spec(&self.workflow_path, &self.workflow_text)
            .map_err(|error| error.to_string())?
            .validate()
            .map(|_| ())
            .map_err(|errors| {
                format!(
                    "scenario workflow {} is invalid: {errors}",
                    self.workflow_path.display()
                )
            })
    }

    pub fn jig_script_path(&self) -> &Path {
        &self.execution.jig_script_path
    }

    pub fn issue(&self, id: &str) -> Result<&IssueFixture, String> {
        self.issues
            .iter()
            .find(|issue| issue.id == id)
            .ok_or_else(|| format!("resolved manifest has no issue fixture `{id}`"))
    }
}

/// Repository fixture declared by the scenario manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoFixture {
    pub id: String,
    pub slug: String,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub seed_path: PathBuf,
    pub ci_source_path: PathBuf,
    pub ci_seed_path: PathBuf,
    pub ci_target: PathBuf,
    pub ci_source: String,
}

/// Issue fixture selected by an `issue.seed` action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueFixture {
    pub id: String,
    pub repo_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// Intake issue fixture declared by the scenario manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeFixture {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// Structured observability settings the live scenario harness applies to the
/// real `temper` processes it spawns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservabilityFixture {
    pub log_format: String,
    pub rust_log: String,
}

impl Default for ObservabilityFixture {
    fn default() -> Self {
        Self {
            log_format: "json".to_string(),
            rust_log: "temper=debug".to_string(),
        }
    }
}

fn validate_fixture_actions(
    execution: &ManifestExecutionPlan,
    workflow_path: &Path,
    repository: &RepoFixture,
) -> Result<(), String> {
    let launch_paths = execution
        .steps
        .iter()
        .filter_map(|step| match &step.action {
            ManifestAction::LaunchTemper { workflow_path } => Some(workflow_path),
            _ => None,
        });
    for declared in launch_paths {
        if declared != workflow_path {
            return Err(format!(
                "temper.launch_standalone config {} does not match workflow.path {}",
                declared.display(),
                workflow_path.display()
            ));
        }
    }
    let seed = execution.steps.iter().find_map(|step| match &step.action {
        ManifestAction::SeedRepository {
            repo_id,
            seed_path,
            ci_source_path,
        } if repo_id == &repository.id => Some((seed_path, ci_source_path)),
        _ => None,
    });
    let Some((seed_path, ci_source_path)) = seed else {
        return Err(format!(
            "resolved repository `{}` has no matching repo.seed action",
            repository.id
        ));
    };
    if seed_path != &repository.seed_path || ci_source_path != &repository.ci_source_path {
        return Err(format!(
            "repo.seed action for `{}` must use the resolved repository seed_path and ci_source",
            repository.id
        ));
    }
    Ok(())
}

fn workflow_fixture(
    scenario_path: &Path,
    manifest: &TomlValue,
) -> Result<(PathBuf, String), String> {
    let workflow = manifest
        .get("workflow")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| "live manifest is missing [workflow]".to_string())?;
    let path = workflow
        .get("path")
        .and_then(TomlValue::as_str)
        .unwrap_or("config/workflow.json");
    let workflow_path = scenario_path.join(path);
    let workflow_text = fs::read_to_string(&workflow_path)
        .map_err(|error| format!("read workflow {}: {error}", workflow_path.display()))?;
    Ok((workflow_path, workflow_text))
}

fn repo_fixture(scenario_path: &Path, manifest: &TomlValue) -> Result<RepoFixture, String> {
    let repos = manifest
        .get("repos")
        .and_then(TomlValue::as_array)
        .ok_or_else(|| "live manifest is missing [[repos]]".to_string())?;
    let repo = repos
        .iter()
        .filter_map(TomlValue::as_table)
        .find(|repo| repo.get("id").and_then(TomlValue::as_str) == Some("service"))
        .or_else(|| repos.iter().filter_map(TomlValue::as_table).next())
        .ok_or_else(|| "live manifest has no repository fixture".to_string())?;
    let id = repo
        .get("id")
        .and_then(TomlValue::as_str)
        .unwrap_or("service")
        .to_string();
    let slug = repo
        .get("slug")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| "live repository fixture is missing `slug`".to_string())?
        .to_string();
    let (owner, name) = split_repo_slug(&slug)?;
    let default_branch = repo
        .get("default_branch")
        .and_then(TomlValue::as_str)
        .unwrap_or("main")
        .to_string();
    let seed_path = scenario_path.join(
        repo.get("seed_path")
            .and_then(TomlValue::as_str)
            .unwrap_or("repo"),
    );
    if !seed_path.is_dir() {
        return Err(format!(
            "repository seed path {} is not a directory",
            seed_path.display()
        ));
    }
    let ci_source_path = scenario_path.join(
        repo.get("ci_source")
            .and_then(TomlValue::as_str)
            .unwrap_or("config/ci.yml"),
    );
    let ci_seed_path = scenario_path.join(
        repo.get("ci_seed_path")
            .and_then(TomlValue::as_str)
            .unwrap_or("repo/.forgejo/workflows/ci.yml"),
    );
    let ci_target = PathBuf::from(
        repo.get("ci_target")
            .and_then(TomlValue::as_str)
            .unwrap_or(".forgejo/workflows/ci.yml"),
    );
    let ci_source = fs::read_to_string(&ci_source_path)
        .map_err(|error| format!("read CI source {}: {error}", ci_source_path.display()))?;
    let ci_seed = fs::read_to_string(&ci_seed_path)
        .map_err(|error| format!("read CI seed {}: {error}", ci_seed_path.display()))?;
    if ci_source != ci_seed {
        return Err(format!(
            "CI source {} and repo seed {} must be byte-equal",
            ci_source_path.display(),
            ci_seed_path.display()
        ));
    }
    Ok(RepoFixture {
        id,
        slug,
        owner,
        name,
        default_branch,
        seed_path,
        ci_source_path,
        ci_seed_path,
        ci_target,
        ci_source,
    })
}

fn split_repo_slug(slug: &str) -> Result<(String, String), String> {
    let mut parts = slug.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(format!(
            "repository slug {slug:?} must be in owner/name form"
        ));
    }
    Ok((owner.to_string(), name.to_string()))
}

fn issue_fixtures(scenario_path: &Path, manifest: &TomlValue) -> Result<Vec<IssueFixture>, String> {
    let issues = manifest
        .get("issues")
        .and_then(TomlValue::as_array)
        .ok_or_else(|| "manifest is missing [[issues]]".to_string())?;
    issues
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let issue = value
                .as_table()
                .ok_or_else(|| format!("issues[{index}] must be a table"))?;
            let required = |field: &str| {
                issue
                    .get(field)
                    .and_then(TomlValue::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| format!("issues[{index}].{field} is required"))
            };
            let body_ref = required("body")?;
            let body_path = scenario_path.join(&body_ref);
            let body = fs::read_to_string(&body_path).map_err(|error| {
                format!(
                    "read issue fixture {} body {}: {error}",
                    issue
                        .get("id")
                        .and_then(TomlValue::as_str)
                        .unwrap_or("<unknown>"),
                    body_path.display()
                )
            })?;
            let labels = issue
                .get("labels")
                .and_then(TomlValue::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .map(|label| {
                            label.as_str().map(str::to_string).ok_or_else(|| {
                                format!("issues[{index}].labels must contain only strings")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            Ok(IssueFixture {
                id: required("id")?,
                repo_id: required("repo")?,
                kind: required("kind")?,
                title: required("title")?,
                body,
                labels,
            })
        })
        .collect()
}

fn intake_fixture(issues: &[IssueFixture]) -> Result<IntakeFixture, String> {
    let issue = issues
        .iter()
        .find(|issue| {
            matches!(issue.kind.as_str(), "intake" | "feature")
                || matches!(issue.id.as_str(), "intake" | "feature")
        })
        .or_else(|| {
            issues.iter().find(|issue| {
                matches!(issue.id.as_str(), "source" | "create") || issue.kind == "code"
            })
        })
        .ok_or_else(|| "manifest has no intake/source issue fixture".to_string())?;
    Ok(IntakeFixture {
        title: issue.title.clone(),
        body: issue.body.clone(),
        labels: issue.labels.clone(),
    })
}

fn observability_fixture(manifest: &TomlValue) -> Result<ObservabilityFixture, String> {
    let defaults = ObservabilityFixture::default();
    let Some(table) = manifest.get("observability").and_then(TomlValue::as_table) else {
        return Ok(defaults);
    };
    let log_format = table
        .get("log_format")
        .map(|value| non_empty_string(value, "observability.log_format"))
        .transpose()?
        .unwrap_or(defaults.log_format);
    if !log_format.trim().eq_ignore_ascii_case("json") {
        return Err(format!(
            "observability.log_format must be `json` for validation-grade live scenario capture, got `{log_format}`"
        ));
    }
    let rust_log = table
        .get("rust_log")
        .map(|value| non_empty_string(value, "observability.rust_log"))
        .transpose()?
        .unwrap_or(defaults.rust_log);
    Ok(ObservabilityFixture {
        log_format,
        rust_log,
    })
}

fn non_empty_string(value: &TomlValue, field: &str) -> Result<String, String> {
    let Some(raw) = value.as_str() else {
        return Err(format!("{field} must be a non-empty string"));
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must be a non-empty string"));
    }
    Ok(trimmed.to_string())
}

fn manifest_duration(
    manifest: &TomlValue,
    key: &str,
    default: Duration,
) -> Result<Duration, String> {
    manifest
        .get(key)
        .map(|value| duration_value(value, key))
        .transpose()
        .map(|duration| duration.unwrap_or(default))
}

fn live_harness_duration(
    manifest: &TomlValue,
    key: &str,
    default: Duration,
) -> Result<Duration, String> {
    manifest
        .get("live_harness")
        .and_then(TomlValue::as_table)
        .and_then(|table| table.get(key))
        .map(|value| duration_value(value, &format!("live_harness.{key}")))
        .transpose()
        .map(|duration| duration.unwrap_or(default))
}

fn duration_value(value: &TomlValue, field: &str) -> Result<Duration, String> {
    if let Some(seconds) = value.as_integer() {
        if seconds < 0 {
            return Err(format!("{field} duration must be non-negative"));
        }
        return Ok(Duration::from_secs(seconds as u64));
    }
    if let Some(raw) = value.as_str() {
        return parse_duration_literal(raw).map_err(|error| format!("{field}: {error}"));
    }
    Err(format!(
        "{field} duration must be an integer seconds value or string literal"
    ))
}

fn parse_duration_literal(raw: &str) -> Result<Duration, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("duration is empty".to_string());
    }
    let (number, multiplier) = if let Some(number) = trimmed.strip_suffix('s') {
        (number, 1)
    } else if let Some(number) = trimmed.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = trimmed.strip_suffix('h') {
        (number, 60 * 60)
    } else {
        (trimmed, 1)
    };
    let amount = number
        .parse::<u64>()
        .map_err(|error| format!("duration {raw:?} is not valid: {error}"))?;
    Ok(Duration::from_secs(amount.saturating_mul(multiplier)))
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
