// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

#[derive(Clone, Debug)]
pub(super) struct IntakeSeed {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) labels: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct RepoSeed {
    pub(super) id: String,
    pub(super) slug: String,
    pub(super) default_branch: String,
}

#[derive(Clone, Debug)]
pub(super) struct Fixture {
    pub(super) workflow_path: PathBuf,
    pub(super) repo: RepoSeed,
    pub(super) intake: IntakeSeed,
}

pub(super) fn load_fixture(scenario_path: &Path, manifest_path: &Path) -> Result<Fixture, String> {
    let manifest = load_manifest_toml(manifest_path)?;
    let workflow_path = workflow_path(scenario_path, &manifest)?;
    let repo = repo_seed(&manifest)?;
    let intake = intake_seed(scenario_path, &manifest)?;
    Ok(Fixture {
        workflow_path,
        repo,
        intake,
    })
}

fn load_manifest_toml(manifest_path: &Path) -> Result<Value, String> {
    load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())
}

fn workflow_path(scenario_path: &Path, manifest: &Value) -> Result<PathBuf, String> {
    let path = manifest
        .get("workflow")
        .and_then(Value::as_table)
        .and_then(|workflow| workflow.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("config/workflow.json");
    Ok(scenario_path.join(path))
}

fn repo_seed(manifest: &Value) -> Result<RepoSeed, String> {
    let repos = manifest
        .get("repos")
        .or_else(|| manifest.get("repositories"))
        .and_then(Value::as_array)
        .ok_or_else(|| "basic-delivery manifest is missing [[repos]]".to_string())?;
    let repo = repos
        .iter()
        .filter_map(Value::as_table)
        .find(|repo| repo.get("id").and_then(Value::as_str) == Some("service"))
        .or_else(|| repos.iter().filter_map(Value::as_table).next())
        .ok_or_else(|| "basic-delivery manifest has no repository fixture".to_string())?;
    let id = repo
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("service")
        .to_string();
    let slug = repo
        .get("slug")
        .or_else(|| repo.get("repo"))
        .or_else(|| repo.get("repository"))
        .and_then(Value::as_str)
        .ok_or_else(|| "basic-delivery repository fixture is missing `slug`".to_string())?
        .to_string();
    let default_branch = repo
        .get("default_branch")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_string();
    Ok(RepoSeed {
        id,
        slug,
        default_branch,
    })
}

fn intake_seed(scenario_path: &Path, manifest: &Value) -> Result<IntakeSeed, String> {
    let issue = manifest
        .get("issues")
        .and_then(Value::as_array)
        .and_then(|issues| {
            issues.iter().filter_map(Value::as_table).find(|issue| {
                issue.get("kind").and_then(Value::as_str) == Some("intake")
                    || issue.get("id").and_then(Value::as_str) == Some("intake")
            })
        })
        .ok_or_else(|| "basic-delivery manifest has no intake issue fixture".to_string())?;
    let title = issue
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| "basic-delivery intake issue is missing `title`".to_string())?
        .to_string();
    let body_ref = issue
        .get("body")
        .and_then(Value::as_str)
        .ok_or_else(|| "basic-delivery intake issue is missing `body`".to_string())?;
    let body_path = scenario_path.join(body_ref);
    let body = fs::read_to_string(&body_path)
        .map_err(|error| format!("read intake body {}: {error}", body_path.display()))?;
    let labels = issue
        .get("labels")
        .and_then(Value::as_array)
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
    Ok(IntakeSeed {
        title,
        body,
        labels,
    })
}
