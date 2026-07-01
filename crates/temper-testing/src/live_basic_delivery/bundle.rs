use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml::Value as TomlValue;

use super::{
    DEFAULT_CONVERGENCE_SECS, DEFAULT_DAEMON_POLL_BACKSTOP_SECS, DEFAULT_MECHANICAL_CADENCE_SECS,
};

/// Scenario-bundle fixtures needed by the live basic-delivery topology.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScenarioBundle {
    pub scenario_path: PathBuf,
    pub manifest_path: PathBuf,
    pub workflow_name: String,
    pub workflow_path: PathBuf,
    pub workflow_text: String,
    pub repo: RepoFixture,
    pub intake: IntakeFixture,
    pub timeout: Duration,
    pub poll_backstop: Duration,
    pub mechanical_cadence: Duration,
}

impl ScenarioBundle {
    /// Loads a basic-delivery scenario directory (or its manifest file) and the
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
        let manifest = load_manifest_toml(&manifest_path)?;
        Self::from_manifest(scenario_path, manifest_path, manifest)
    }

    /// Builds a live basic-delivery bundle from an already-resolved manifest.
    /// Callers that support fixture inheritance can pass a manifest whose local
    /// path strings have already been rewritten to the files that declared them.
    pub fn from_manifest(
        scenario_path: PathBuf,
        manifest_path: PathBuf,
        manifest: TomlValue,
    ) -> Result<Self, String> {
        let (workflow_name, workflow_path, workflow_text) =
            workflow_fixture(&scenario_path, &manifest)?;
        let repo = repo_fixture(&scenario_path, &manifest)?;
        let intake = intake_fixture(&scenario_path, &manifest)?;
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

        Ok(Self {
            scenario_path,
            manifest_path,
            workflow_name,
            workflow_path,
            workflow_text,
            repo,
            intake,
            timeout,
            poll_backstop,
            mechanical_cadence,
        })
    }

    /// Validates that the scenario workflow fixture is the canonical bundled
    /// basic-delivery workflow. This preserves the live proof's contract with
    /// `temper init --workflow basic-delivery` while letting the harness load the
    /// bytes from the scenario bundle.
    pub fn assert_workflow_matches_reference(&self) -> Result<(), String> {
        let spec = temper_reference_delivery::parse_workflow_spec(
            &self.workflow_path,
            &self.workflow_text,
        )
        .map_err(|error| error.to_string())?;
        let validated = spec.validate().map_err(|errors| {
            format!(
                "scenario workflow {} is invalid: {errors}",
                self.workflow_path.display()
            )
        })?;
        if self.workflow_name == "basic-delivery" {
            let reference = temper_reference_delivery::basic_delivery_workflow();
            if validated != reference {
                return Err(format!(
                    "scenario workflow {} no longer validates to the canonical basic-delivery workflow",
                    self.workflow_path.display()
                ));
            }
            if self.workflow_text != temper_reference_delivery::basic_delivery_workflow_json() {
                return Err(format!(
                    "scenario workflow {} must stay byte-equal to the embedded basic-delivery fixture",
                    self.workflow_path.display()
                ));
            }
        }
        Ok(())
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

/// Intake issue fixture declared by the scenario manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeFixture {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

fn load_manifest_toml(manifest_path: &Path) -> Result<TomlValue, String> {
    let source = fs::read_to_string(manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    source
        .parse::<TomlValue>()
        .map_err(|error| format!("parse {}: {error}", manifest_path.display()))
}

fn workflow_fixture(
    scenario_path: &Path,
    manifest: &TomlValue,
) -> Result<(String, PathBuf, String), String> {
    let workflow = manifest
        .get("workflow")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| "basic-delivery manifest is missing [workflow]".to_string())?;
    let name = workflow
        .get("name")
        .and_then(TomlValue::as_str)
        .unwrap_or("basic-delivery")
        .to_string();
    let path = workflow
        .get("path")
        .and_then(TomlValue::as_str)
        .unwrap_or("config/workflow.json");
    let workflow_path = scenario_path.join(path);
    let workflow_text = fs::read_to_string(&workflow_path)
        .map_err(|error| format!("read workflow {}: {error}", workflow_path.display()))?;
    Ok((name, workflow_path, workflow_text))
}

fn repo_fixture(scenario_path: &Path, manifest: &TomlValue) -> Result<RepoFixture, String> {
    let repos = manifest
        .get("repos")
        .and_then(TomlValue::as_array)
        .ok_or_else(|| "basic-delivery manifest is missing [[repos]]".to_string())?;
    let repo = repos
        .iter()
        .filter_map(TomlValue::as_table)
        .find(|repo| repo.get("id").and_then(TomlValue::as_str) == Some("service"))
        .or_else(|| repos.iter().filter_map(TomlValue::as_table).next())
        .ok_or_else(|| "basic-delivery manifest has no repository fixture".to_string())?;
    let id = repo
        .get("id")
        .and_then(TomlValue::as_str)
        .unwrap_or("service")
        .to_string();
    let slug = repo
        .get("slug")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| "basic-delivery repository fixture is missing `slug`".to_string())?
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

fn intake_fixture(scenario_path: &Path, manifest: &TomlValue) -> Result<IntakeFixture, String> {
    let issue = manifest
        .get("issues")
        .and_then(TomlValue::as_array)
        .and_then(|issues| {
            issues.iter().filter_map(TomlValue::as_table).find(|issue| {
                issue.get("kind").and_then(TomlValue::as_str) == Some("intake")
                    || issue.get("id").and_then(TomlValue::as_str) == Some("intake")
            })
        })
        .ok_or_else(|| "basic-delivery manifest has no intake issue fixture".to_string())?;
    let title = issue
        .get("title")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| "basic-delivery intake issue is missing `title`".to_string())?
        .to_string();
    let body_ref = issue
        .get("body")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| "basic-delivery intake issue is missing `body`".to_string())?;
    let body_path = scenario_path.join(body_ref);
    let body = fs::read_to_string(&body_path)
        .map_err(|error| format!("read intake body {}: {error}", body_path.display()))?;
    let labels = issue
        .get("labels")
        .and_then(TomlValue::as_array)
        .map(|labels| {
            labels
                .iter()
                .map(|label| {
                    label.as_str().map(str::to_string).ok_or_else(|| {
                        "basic-delivery intake issue labels must be strings".to_string()
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(IntakeFixture {
        title,
        body,
        labels,
    })
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
mod tests {
    use super::*;

    #[test]
    fn loads_checked_in_live_basic_delivery_bundle() {
        let bundle = ScenarioBundle::load(default_basic_delivery_scenario_path())
            .expect("checked-in basic-delivery scenario bundle loads");
        assert_eq!(bundle.workflow_name, "basic-delivery");
        assert_eq!(bundle.repo.slug, "acme/service");
        assert_eq!(bundle.repo.default_branch, "main");
        assert_eq!(
            bundle.intake.title,
            "Service banner should identify the environment"
        );
        assert_eq!(bundle.timeout, Duration::from_secs(600));
        assert_eq!(
            bundle.poll_backstop,
            Duration::from_secs(DEFAULT_DAEMON_POLL_BACKSTOP_SECS)
        );
        assert_eq!(
            bundle.mechanical_cadence,
            Duration::from_secs(DEFAULT_MECHANICAL_CADENCE_SECS)
        );
        assert!(bundle.repo.seed_path.join("README.md").is_file());
        assert!(
            bundle
                .repo
                .seed_path
                .join(".forgejo/workflows/ci.yml")
                .is_file()
        );
        assert_eq!(
            bundle.repo.ci_source,
            fs::read_to_string(&bundle.repo.ci_seed_path).unwrap()
        );
        bundle
            .assert_workflow_matches_reference()
            .expect("scenario workflow remains canonical");
    }

    fn default_basic_delivery_scenario_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("temper-testing lives under crates/temper-testing")
            .join("scenarios/basic-delivery")
    }
}
