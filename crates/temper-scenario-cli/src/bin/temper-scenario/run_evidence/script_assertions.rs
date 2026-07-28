// SPDX-License-Identifier: MPL-2.0

#[path = "script_assertions/execution.rs"]
mod execution;

use std::fs;
use std::path::{Component, Path, PathBuf};

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::model::{
    ASSERTION_STATUS_FAILED, AssertionEvidence, AssertionResultEvidence, RunEvidenceArtifact,
};
use execution::run_hook;

pub(super) const SCRIPT_CONTEXT_SCHEMA: &str = "temper.scenario.script-assertion-context";
pub(super) const SCRIPT_CONTEXT_VERSION: u64 = 1;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const SCRIPT_PHASE_AFTER_CONVERGENCE: &str = "after-convergence";
pub(super) const SCRIPT_KIND_COMMAND: &str = "command";
const HOOK_ARTIFACT_DIR: &str = "script-assertions";

pub(crate) fn append_script_assertions(
    manifest_path: &Path,
    artifact: &mut RunEvidenceArtifact,
    artifact_dir: &Path,
) -> Result<(), String> {
    let manifest = load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())?;
    let Some(assertions) = manifest.get("assertions") else {
        return Ok(());
    };
    let Some(hooks) = assertions.as_array() else {
        let hook_dir = artifact_dir.join(HOOK_ARTIFACT_DIR).join("assertions");
        let result = config_error_result(
            "assertions".to_string(),
            true,
            None,
            None,
            None,
            &hook_dir,
            "assertions must be an array of tables declared with [[assertions]]".to_string(),
        );
        register_result_artifacts(artifact, &result);
        append_result(artifact, result);
        return Ok(());
    };

    for (index, value) in hooks.iter().enumerate() {
        let table = value.as_table();
        let id = table
            .and_then(|table| table.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("assertions[{index}]"));
        let hook_dir = artifact_dir
            .join(HOOK_ARTIFACT_DIR)
            .join(format!("{index:02}-{}", safe_file_component(&id)));

        let required = table
            .and_then(|table| table.get("required"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let result = match table {
            Some(table) => match parse_hook(index, &id, table, artifact) {
                Ok(hook) => run_hook(&hook, artifact, artifact_dir, &hook_dir),
                Err(message) => Ok(config_error_result(
                    id,
                    required,
                    table
                        .get("kind")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    table
                        .get("phase")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    table
                        .get("command")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    &hook_dir,
                    message,
                )),
            },
            None => Ok(config_error_result(
                id,
                true,
                None,
                None,
                None,
                &hook_dir,
                "assertion hook entries must be tables".to_string(),
            )),
        }?;
        register_result_artifacts(artifact, &result);
        append_result(artifact, result);
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct ScriptHook {
    pub(super) id: String,
    pub(super) required: bool,
    pub(super) phase: String,
    pub(super) command: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) timeout_ms: u64,
    pub(super) env_allow: Vec<String>,
}

fn parse_hook(
    index: usize,
    id: &str,
    table: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> Result<ScriptHook, String> {
    let kind = required_string(table, "kind", index)?;
    if kind != SCRIPT_KIND_COMMAND {
        return Err(format!(
            "kind must be `{SCRIPT_KIND_COMMAND}` for shell assertion hooks (got `{kind}`)"
        ));
    }

    let phase = optional_string(table, "phase", index)?
        .unwrap_or_else(|| SCRIPT_PHASE_AFTER_CONVERGENCE.to_string());
    if phase != SCRIPT_PHASE_AFTER_CONVERGENCE {
        return Err(format!(
            "phase must be `{SCRIPT_PHASE_AFTER_CONVERGENCE}` for command assertion hooks (got `{phase}`)"
        ));
    }

    let command_raw = required_string(table, "command", index)?;
    validate_localish_path(&command_raw, "command")?;
    let command = PathBuf::from(&command_raw);
    if !command.is_file() {
        return Err(format!(
            "command path is not a readable file after manifest resolution: {}",
            command.display()
        ));
    }

    let cwd = match optional_string(table, "cwd", index)? {
        Some(raw) => {
            validate_localish_path(&raw, "cwd")?;
            PathBuf::from(raw)
        }
        None => PathBuf::from(&artifact.scenario.scenario_path),
    };
    if !cwd.is_dir() {
        return Err(format!(
            "cwd is not a readable directory after manifest resolution: {}",
            cwd.display()
        ));
    }

    Ok(ScriptHook {
        id: id.to_string(),
        required: optional_bool(table, "required", index)?.unwrap_or(true),
        phase,
        command,
        cwd,
        timeout_ms: timeout_ms(table, index)?,
        env_allow: env_allowlist(table, index)?,
    })
}

fn config_error_result(
    id: String,
    required: bool,
    kind: Option<String>,
    phase: Option<String>,
    command: Option<String>,
    hook_dir: &Path,
    message: String,
) -> AssertionResultEvidence {
    let _ = fs::create_dir_all(hook_dir);
    let status_path = hook_dir.join("config-error.txt");
    let _ = fs::write(&status_path, format!("{message}\n"));
    AssertionResultEvidence {
        id,
        required,
        status: ASSERTION_STATUS_FAILED.to_string(),
        description: "Script assertion hook configuration is invalid.".to_string(),
        artifact: None,
        kind,
        phase,
        command,
        cwd: None,
        context_path: None,
        stdout_path: None,
        stderr_path: None,
        status_path: Some(status_path.display().to_string()),
        exit_status: None,
        timeout_ms: None,
        duration_ms: None,
        details: vec![
            message,
            format!("config diagnostic: `{}`", status_path.display()),
        ],
    }
}

fn append_result(artifact: &mut RunEvidenceArtifact, result: AssertionResultEvidence) {
    if let Some(assertions) = artifact.assertions.as_mut() {
        assertions.append_result(result);
        artifact.verdict = assertions.verdict();
    } else {
        artifact.record_assertions(AssertionEvidence::from_results(vec![result]));
    }
}

fn register_result_artifacts(artifact: &mut RunEvidenceArtifact, result: &AssertionResultEvidence) {
    if let Some(path) = result.context_path.as_deref() {
        push_unique(&mut artifact.artifacts.artifact_paths, path.to_string());
    }
    if let Some(path) = result.status_path.as_deref() {
        push_unique(&mut artifact.artifacts.artifact_paths, path.to_string());
    }
    if let Some(path) = result.stdout_path.as_deref() {
        push_unique(&mut artifact.artifacts.log_paths, path.to_string());
    }
    if let Some(path) = result.stderr_path.as_deref() {
        push_unique(&mut artifact.artifacts.log_paths, path.to_string());
    }
}

fn push_unique(paths: &mut Vec<String>, path: String) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn required_string(table: &toml::Table, field: &str, index: usize) -> Result<String, String> {
    let Some(value) = table.get(field) else {
        return Err(format!("assertions[{index}].{field} is required"));
    };
    let Some(value) = value.as_str() else {
        return Err(format!("assertions[{index}].{field} must be a string"));
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("assertions[{index}].{field} must not be empty"));
    }
    Ok(trimmed.to_string())
}

fn optional_string(
    table: &toml::Table,
    field: &str,
    index: usize,
) -> Result<Option<String>, String> {
    let Some(value) = table.get(field) else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(format!("assertions[{index}].{field} must be a string"));
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("assertions[{index}].{field} must not be empty"));
    }
    Ok(Some(trimmed.to_string()))
}

fn optional_bool(table: &toml::Table, field: &str, index: usize) -> Result<Option<bool>, String> {
    let Some(value) = table.get(field) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| format!("assertions[{index}].{field} must be a boolean"))
}

fn timeout_ms(table: &toml::Table, index: usize) -> Result<u64, String> {
    let Some(value) = table.get("timeout_ms") else {
        return Ok(DEFAULT_TIMEOUT_MS);
    };
    let Some(timeout) = value.as_integer().filter(|timeout| *timeout > 0) else {
        return Err(format!(
            "assertions[{index}].timeout_ms must be a positive integer"
        ));
    };
    let timeout = u64::try_from(timeout).map_err(|_| {
        format!("assertions[{index}].timeout_ms must fit in an unsigned 64-bit integer")
    })?;
    if timeout > MAX_TIMEOUT_MS {
        return Err(format!(
            "assertions[{index}].timeout_ms must be <= {MAX_TIMEOUT_MS}"
        ));
    }
    Ok(timeout)
}

fn env_allowlist(table: &toml::Table, index: usize) -> Result<Vec<String>, String> {
    let Some(value) = table.get("env") else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(format!(
            "assertions[{index}].env must be an array of variable names"
        ));
    };
    let mut names = Vec::new();
    for (env_index, item) in items.iter().enumerate() {
        let Some(name) = item.as_str() else {
            return Err(format!(
                "assertions[{index}].env[{env_index}] must be a string"
            ));
        };
        let name = name.trim();
        if !is_env_name(name) {
            return Err(format!(
                "assertions[{index}].env[{env_index}] must be an ASCII environment variable name"
            ));
        }
        if is_reserved_env(name) {
            return Err(format!(
                "assertions[{index}].env[{env_index}] must not override Temper-managed environment variables"
            ));
        }
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn validate_localish_path(raw: &str, field: &str) -> Result<(), String> {
    if raw.contains("://") {
        return Err(format!(
            "{field} must be a local filesystem path, not a URL"
        ));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!(
            "{field} must be a normalized local path without `..` components"
        ));
    }
    Ok(())
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn is_reserved_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(name, "PATH" | "LC_ALL")
        || name.starts_with("TEMPER_SCENARIO_")
        || [
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "CREDENTIAL",
            "AUTH",
            "API_KEY",
        ]
        .iter()
        .any(|marker| upper.contains(marker))
}

fn safe_file_component(value: &str) -> String {
    let safe = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "assertion".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_assertions_cannot_import_or_override_credentials() {
        for name in [
            "FORGEJO_TOKEN",
            "API_SECRET",
            "USER_PASSWORD",
            "AWS_CREDENTIAL_FILE",
            "HTTP_AUTH",
            "OPENAI_API_KEY",
            "TEMPER_SCENARIO_CONTEXT",
        ] {
            assert!(is_reserved_env(name), "{name}");
        }
        assert!(!is_reserved_env("SCENARIO_EXPECTED_VALUE"));
    }
}
