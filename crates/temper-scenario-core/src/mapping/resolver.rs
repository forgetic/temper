// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::{CheckReport, ScenarioStatus, check_scenarios, scenario_content_digest};

use super::{
    FEATURE_SCENARIO_MAPPING_SCHEMA, FeatureMappingChange, FeatureScenarioBaseComparison,
    FeatureScenarioResolveError, ForgeIssueKey, ResolveFeatureScenarioRequest,
    ResolvedFeatureScenario, validate_source_branch,
};

/// Select exactly one active scenario that explicitly maps `request.feature`.
pub fn resolve_feature_scenario(
    request: &ResolveFeatureScenarioRequest,
) -> Result<ResolvedFeatureScenario, FeatureScenarioResolveError> {
    validate_request(request)?;
    let checkout = canonical_directory(&request.checkout_root, "checkout root")?;
    let scenarios = scenarios_directory(request, &checkout)?;
    let reports = valid_reports(&scenarios)?;
    let report = unique_match(&reports, request, &checkout, &scenarios)?;
    let manifest = report
        .manifest
        .as_ref()
        .ok_or_else(|| invalid_manifest_error(report))?;
    let mapping = manifest
        .feature_mapping
        .as_ref()
        .expect("matching report has feature mapping");
    let scenario_path = relative_display(&checkout, &report.scenario_path);
    if manifest.status != ScenarioStatus::Active {
        return Err(FeatureScenarioResolveError::Inactive {
            path: scenario_path,
            status: manifest.status,
        });
    }
    validate_mapped_path(&checkout, &scenarios, report, &manifest.name)?;

    let manifest_path = report
        .manifest_path
        .as_deref()
        .ok_or_else(|| invalid_manifest_error(report))?;
    let manifest_relative = relative_display(&checkout, manifest_path);
    let head_sha = git_output(&checkout, &["rev-parse", "--verify", "HEAD"])?;
    let base_revision = format!("{}^{{commit}}", request.landing_base);
    let landing_base_sha = git_output(
        &checkout,
        &["rev-parse", "--verify", base_revision.as_str()],
    )?;
    reject_dirty_scenario(
        &checkout,
        &relative_display(&checkout, &scenarios),
        &scenario_path,
    )?;

    let path_existed = git_path_exists(&checkout, &landing_base_sha, &manifest_relative)?;
    let base_had_mapping = base_contains_mapping(
        &checkout,
        &landing_base_sha,
        &relative_display(&checkout, &scenarios),
        &request.feature,
    )?;
    let existed = path_existed || base_had_mapping;
    let base_comparison = validate_change_intent(
        &scenario_path,
        &request.landing_base,
        existed,
        mapping.change,
    )?;
    let content_changed_from_base = if path_existed {
        git_path_changed(
            &checkout,
            &landing_base_sha,
            &head_sha,
            &relative_display(&checkout, &report.scenario_path),
        )?
    } else {
        true
    };
    if existed && !content_changed_from_base {
        return Err(FeatureScenarioResolveError::Unchanged {
            path: scenario_path,
            base: request.landing_base.clone(),
        });
    }

    let digest = scenario_content_digest(report)
        .map_err(|error| FeatureScenarioResolveError::Digest(error.to_string()))?;
    if let Some(expected) = request.expected_digest.as_deref() {
        if expected != digest {
            return Err(FeatureScenarioResolveError::DigestMismatch {
                expected: expected.to_string(),
                actual: digest,
            });
        }
    }

    Ok(ResolvedFeatureScenario {
        schema: FEATURE_SCENARIO_MAPPING_SCHEMA.to_string(),
        mapping_id: mapping.identity(&manifest.name),
        feature: request.feature.clone(),
        plan: mapping.plan.clone(),
        scenario_name: manifest.name.clone(),
        scenario_path,
        manifest_path: manifest_relative,
        source_branch: mapping.source_branch.clone(),
        head_sha,
        landing_base: request.landing_base.clone(),
        landing_base_sha,
        base_comparison,
        content_changed_from_base,
        change_intent: mapping.change,
        digest,
    })
}

fn validate_request(
    request: &ResolveFeatureScenarioRequest,
) -> Result<(), FeatureScenarioResolveError> {
    let landing_base = request.landing_base.trim();
    if landing_base != request.landing_base {
        return Err(FeatureScenarioResolveError::InvalidInput(
            "landing base must not contain surrounding whitespace".to_string(),
        ));
    }
    if let Err(message) = validate_source_branch(landing_base) {
        return Err(FeatureScenarioResolveError::InvalidInput(format!(
            "landing base must be a safe Git revision: {message}"
        )));
    }
    if let Some(expected) = request.expected_digest.as_deref() {
        validate_digest(expected).map_err(FeatureScenarioResolveError::InvalidInput)?;
    }
    Ok(())
}

fn scenarios_directory(
    request: &ResolveFeatureScenarioRequest,
    checkout: &Path,
) -> Result<PathBuf, FeatureScenarioResolveError> {
    let input = if request.scenarios_root.is_absolute() {
        request.scenarios_root.clone()
    } else {
        checkout.join(&request.scenarios_root)
    };
    let scenarios = canonical_directory(&input, "scenarios root")?;
    if !scenarios.starts_with(checkout) {
        return Err(FeatureScenarioResolveError::InvalidInput(format!(
            "scenarios root {} escapes checkout {}",
            scenarios.display(),
            checkout.display()
        )));
    }
    Ok(scenarios)
}

fn valid_reports(scenarios: &Path) -> Result<Vec<CheckReport>, FeatureScenarioResolveError> {
    let reports = check_scenarios(scenarios)
        .map_err(|error| FeatureScenarioResolveError::InvalidInput(error.to_string()))?;
    for report in &reports {
        if !report.is_valid() {
            return Err(invalid_manifest_error(report));
        }
    }
    Ok(reports)
}

fn unique_match<'a>(
    reports: &'a [CheckReport],
    request: &ResolveFeatureScenarioRequest,
    checkout: &Path,
    scenarios: &Path,
) -> Result<&'a CheckReport, FeatureScenarioResolveError> {
    let mut matches = reports
        .iter()
        .filter(|report| {
            report
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.feature_mapping.as_ref())
                .is_some_and(|mapping| mapping.feature == request.feature)
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.scenario_path.cmp(&right.scenario_path));
    match matches.as_slice() {
        [] => Err(FeatureScenarioResolveError::Missing {
            feature: request.feature.clone(),
            root: relative_display(checkout, scenarios),
        }),
        [report] => Ok(*report),
        many => Err(FeatureScenarioResolveError::Ambiguous {
            feature: request.feature.clone(),
            paths: many
                .iter()
                .map(|report| relative_display(checkout, &report.scenario_path))
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

fn validate_change_intent(
    path: &str,
    base: &str,
    existed: bool,
    actual: FeatureMappingChange,
) -> Result<FeatureScenarioBaseComparison, FeatureScenarioResolveError> {
    if existed {
        if actual != FeatureMappingChange::Updated {
            return Err(FeatureScenarioResolveError::UpdatedIntent {
                path: path.to_string(),
                base: base.to_string(),
                actual,
            });
        }
        Ok(FeatureScenarioBaseComparison::Updated)
    } else {
        if actual != FeatureMappingChange::New {
            return Err(FeatureScenarioResolveError::NewIntent {
                path: path.to_string(),
                base: base.to_string(),
                actual,
            });
        }
        Ok(FeatureScenarioBaseComparison::New)
    }
}

fn reject_dirty_scenario(
    checkout: &Path,
    status_scope: &str,
    mapped_path: &str,
) -> Result<(), FeatureScenarioResolveError> {
    let dirty = git_output(
        checkout,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            status_scope,
        ],
    )?;
    if dirty.is_empty() {
        Ok(())
    } else {
        Err(FeatureScenarioResolveError::Dirty {
            path: mapped_path.to_string(),
        })
    }
}

fn validate_digest(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err("expected digest must use `sha256:<64 lowercase hex characters>`".to_string());
    };
    if hex.len() != 64
        || !hex
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        return Err("expected digest must use `sha256:<64 lowercase hex characters>`".to_string());
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, FeatureScenarioResolveError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        FeatureScenarioResolveError::InvalidInput(format!(
            "cannot canonicalize {label} {}: {error}",
            path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(FeatureScenarioResolveError::InvalidInput(format!(
            "{label} is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn invalid_manifest_error(report: &CheckReport) -> FeatureScenarioResolveError {
    FeatureScenarioResolveError::InvalidManifest {
        path: report
            .manifest_path
            .as_deref()
            .unwrap_or(&report.scenario_path)
            .display()
            .to_string(),
        diagnostics: report
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
    }
}

fn validate_mapped_path(
    checkout: &Path,
    scenarios: &Path,
    report: &CheckReport,
    scenario_name: &str,
) -> Result<(), FeatureScenarioResolveError> {
    let canonical = fs::canonicalize(&report.scenario_path).map_err(|error| {
        FeatureScenarioResolveError::Unsafe {
            path: relative_display(checkout, &report.scenario_path),
            reason: format!("cannot canonicalize scenario directory: {error}"),
        }
    })?;
    let direct_parent = canonical.parent() == Some(scenarios);
    let directory_name = canonical.file_name().and_then(|name| name.to_str());
    let metadata = fs::symlink_metadata(&report.scenario_path).map_err(|error| {
        FeatureScenarioResolveError::Unsafe {
            path: relative_display(checkout, &report.scenario_path),
            reason: format!("cannot inspect scenario directory: {error}"),
        }
    })?;
    if !canonical.starts_with(scenarios) || !direct_parent || metadata.file_type().is_symlink() {
        return Err(FeatureScenarioResolveError::Unsafe {
            path: relative_display(checkout, &report.scenario_path),
            reason: "scenario must be a direct, non-symlink child of the configured scenarios root"
                .to_string(),
        });
    }
    if directory_name != Some(scenario_name) {
        return Err(FeatureScenarioResolveError::Unsafe {
            path: relative_display(checkout, &report.scenario_path),
            reason: format!(
                "manifest name `{scenario_name}` must match its directory name `{}`",
                directory_name.unwrap_or("<non-UTF-8>")
            ),
        });
    }
    Ok(())
}

fn base_contains_mapping(
    checkout: &Path,
    base_sha: &str,
    scenarios_relative: &str,
    feature: &ForgeIssueKey,
) -> Result<bool, FeatureScenarioResolveError> {
    let output = git_output(
        checkout,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            base_sha,
            "--",
            scenarios_relative,
        ],
    )?;
    for path in output.lines().filter(|path| {
        crate::MANIFEST_FILE_NAMES
            .iter()
            .any(|name| path.ends_with(&format!("/{name}")))
    }) {
        let object = format!("{base_sha}:{path}");
        let source = git_output(checkout, &["show", object.as_str()])?;
        let Ok(value) = source.parse::<toml::Value>() else {
            continue;
        };
        let mapped = value
            .get("validation")
            .and_then(toml::Value::as_table)
            .and_then(|validation| validation.get("feature"))
            .and_then(toml::Value::as_str)
            .and_then(|value| value.parse::<ForgeIssueKey>().ok());
        if mapped.as_ref() == Some(feature) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn git_path_exists(
    checkout: &Path,
    revision: &str,
    path: &str,
) -> Result<bool, FeatureScenarioResolveError> {
    let object = format!("{revision}:{path}");
    let status = git_status(checkout, &["cat-file", "-e", object.as_str()])?;
    if status.success() {
        Ok(true)
    } else if status.code() == Some(1) || status.code() == Some(128) {
        Ok(false)
    } else {
        Err(FeatureScenarioResolveError::Git(format!(
            "git cat-file exited with {status} for {object}"
        )))
    }
}

fn git_path_changed(
    checkout: &Path,
    base_sha: &str,
    head_sha: &str,
    path: &str,
) -> Result<bool, FeatureScenarioResolveError> {
    let status = git_status(
        checkout,
        &["diff", "--quiet", base_sha, head_sha, "--", path],
    )?;
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(FeatureScenarioResolveError::Git(format!(
            "git diff exited with {status} for {path}"
        ))),
    }
}

fn git_status(checkout: &Path, args: &[&str]) -> Result<ExitStatus, FeatureScenarioResolveError> {
    Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            FeatureScenarioResolveError::Git(format!(
                "failed to run `git {}`: {error}",
                args.join(" ")
            ))
        })
}

fn git_output(checkout: &Path, args: &[&str]) -> Result<String, FeatureScenarioResolveError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| {
            FeatureScenarioResolveError::Git(format!(
                "failed to run `git {}`: {error}",
                args.join(" ")
            ))
        })?;
    if !output.status.success() {
        return Err(FeatureScenarioResolveError::Git(format!(
            "`git {}` exited with {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn relative_display(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            Component::CurDir => None,
            _ => Some(component.as_os_str().to_string_lossy().to_string()),
        })
        .collect::<Vec<_>>()
        .join("/")
}
