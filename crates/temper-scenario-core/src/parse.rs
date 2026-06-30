// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;

use toml::Value;

use crate::issue_refs::collect_issue_references;
use crate::path_refs::collect_path_references;
use crate::repo_refs::{
    collect_repository_references, repository_aliases, validate_repository_fields,
};
use crate::toml_helpers::string_value;
use crate::{
    Diagnostic, ManifestLoadError, PathReference, ScenarioIntent, ScenarioManifest,
    ScenarioStability, ScenarioStatus, Severity,
};

/// Loads, parses, and validates a single manifest file.
pub fn load_manifest(path: impl AsRef<Path>) -> Result<ScenarioManifest, ManifestLoadError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| ManifestLoadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let value = source
        .parse::<Value>()
        .map_err(|source| ManifestLoadError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let (manifest, diagnostics) = parse_manifest_value(&value, base_dir);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(ManifestLoadError::Invalid {
            path: path.to_path_buf(),
            diagnostics,
        });
    }
    manifest.ok_or_else(|| ManifestLoadError::Invalid {
        path: path.to_path_buf(),
        diagnostics: vec![Diagnostic::document_error(
            "manifest did not produce required scenario metadata",
        )],
    })
}

/// Parses and validates a manifest document string relative to `base_dir`.
pub fn parse_manifest_str(
    source: &str,
    base_dir: impl AsRef<Path>,
) -> Result<ScenarioManifest, Vec<Diagnostic>> {
    let value = match source.parse::<Value>() {
        Ok(value) => value,
        Err(error) => {
            return Err(vec![Diagnostic::document_error(format!(
                "invalid TOML: {error}"
            ))]);
        }
    };
    let (manifest, diagnostics) = parse_manifest_value(&value, base_dir.as_ref());
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        Err(diagnostics)
    } else if let Some(manifest) = manifest {
        Ok(manifest)
    } else {
        Err(vec![Diagnostic::document_error(
            "manifest did not produce required scenario metadata",
        )])
    }
}

pub(crate) fn parse_manifest_value(
    value: &Value,
    base_dir: &Path,
) -> (Option<ScenarioManifest>, Vec<Diagnostic>) {
    let Some(table) = value.as_table() else {
        return (
            None,
            vec![Diagnostic::document_error(
                "manifest root must be a TOML table",
            )],
        );
    };
    let scenario = table.get("scenario").and_then(Value::as_table);
    let mut diagnostics = Vec::new();

    validate_schema_version(table, &mut diagnostics);

    let name = required_metadata_string(table, scenario, "name", &mut diagnostics);
    let status = parse_status(table, scenario, &mut diagnostics);
    let stability = parse_stability(table, scenario, &mut diagnostics);
    let intent = parse_intent(table, scenario, &mut diagnostics);

    let mut path_references = Vec::<PathReference>::new();
    collect_path_references(value, "", base_dir, &mut path_references, &mut diagnostics);

    let repositories = collect_repository_references(value, &mut diagnostics);
    let aliases = repository_aliases(&repositories);
    validate_repository_fields(value, "", &aliases, &mut diagnostics);
    let issues = collect_issue_references(value, &aliases, repositories.len(), &mut diagnostics);

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return (None, diagnostics);
    }

    let manifest = match (name, status, stability, intent) {
        (Some(name), Some(status), Some(stability), Some(intent)) => Some(ScenarioManifest {
            name,
            status,
            stability,
            intent,
            repositories,
            issues,
            path_references,
        }),
        _ => None,
    };
    (manifest, diagnostics)
}

fn validate_schema_version(table: &toml::Table, diagnostics: &mut Vec<Diagnostic>) {
    if let Some(value) = table.get("schema") {
        match value.as_str() {
            Some("temper.scenario.v1") => {}
            Some(other) => diagnostics.push(Diagnostic::error(
                "schema",
                format!("unsupported schema `{other}` (expected `temper.scenario.v1`)"),
            )),
            None => diagnostics.push(Diagnostic::error(
                "schema",
                "must be the string `temper.scenario.v1`",
            )),
        }
    }

    let Some((field, value)) = table
        .get("schema_version")
        .map(|value| ("schema_version", value))
        .or_else(|| {
            table
                .get("manifest_version")
                .map(|value| ("manifest_version", value))
        })
    else {
        return;
    };
    match value.as_integer() {
        Some(1) => {}
        Some(other) => diagnostics.push(Diagnostic::error(
            field,
            format!("unsupported schema version {other}; expected 1"),
        )),
        None => diagnostics.push(Diagnostic::error(field, "must be the integer 1")),
    }
}

fn required_metadata_string(
    table: &toml::Table,
    scenario: Option<&toml::Table>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let Some((field, value)) = metadata_value(table, scenario, key) else {
        diagnostics.push(Diagnostic::error(key, "required field is missing"));
        return None;
    };
    string_value(field, value, diagnostics)
}

fn parse_status(
    table: &toml::Table,
    scenario: Option<&toml::Table>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ScenarioStatus> {
    let raw = required_metadata_string(table, scenario, "status", diagnostics)?;
    match ScenarioStatus::parse(&raw) {
        Some(status) => Some(status),
        None => {
            diagnostics.push(Diagnostic::error(
                metadata_field_name(scenario, "status"),
                format!(
                    "unknown status `{raw}` (expected one of: {})",
                    ScenarioStatus::allowed_values().join(", ")
                ),
            ));
            None
        }
    }
}

fn parse_stability(
    table: &toml::Table,
    scenario: Option<&toml::Table>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ScenarioStability> {
    let raw = required_metadata_string(table, scenario, "stability", diagnostics)?;
    match ScenarioStability::parse(&raw) {
        Some(stability) => Some(stability),
        None => {
            diagnostics.push(Diagnostic::error(
                metadata_field_name(scenario, "stability"),
                format!(
                    "unknown stability `{raw}` (expected one of: {})",
                    ScenarioStability::allowed_values().join(", ")
                ),
            ));
            None
        }
    }
}

fn parse_intent(
    table: &toml::Table,
    scenario: Option<&toml::Table>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ScenarioIntent> {
    let Some((field, value)) = metadata_value(table, scenario, "intent") else {
        diagnostics.push(Diagnostic::error("intent", "required field is missing"));
        return None;
    };

    if value.is_str() {
        return string_value(field.clone(), value, diagnostics).map(|summary| ScenarioIntent {
            summary: Some(summary),
            path: None,
        });
    }

    let Some(intent_table) = value.as_table() else {
        diagnostics.push(Diagnostic::error(
            field,
            "must be a non-empty string or a table with `summary`/`text` or `path`",
        ));
        return None;
    };

    let summary = intent_table
        .get("summary")
        .map(|value| string_value(format!("{field}.summary"), value, diagnostics))
        .or_else(|| {
            intent_table
                .get("text")
                .map(|value| string_value(format!("{field}.text"), value, diagnostics))
        })
        .flatten();
    let path = intent_table
        .get("path")
        .and_then(|value| string_value(format!("{field}.path"), value, diagnostics));

    if summary.is_none() && path.is_none() {
        diagnostics.push(Diagnostic::error(
            field,
            "must include a non-empty `summary`, `text`, or `path`",
        ));
        return None;
    }

    Some(ScenarioIntent { summary, path })
}

fn metadata_value<'a>(
    table: &'a toml::Table,
    scenario: Option<&'a toml::Table>,
    key: &str,
) -> Option<(String, &'a Value)> {
    scenario
        .and_then(|scenario| {
            scenario
                .get(key)
                .map(|value| (format!("scenario.{key}"), value))
        })
        .or_else(|| table.get(key).map(|value| (key.to_string(), value)))
}

fn metadata_field_name(scenario: Option<&toml::Table>, key: &str) -> String {
    if scenario.is_some_and(|scenario| scenario.contains_key(key)) {
        format!("scenario.{key}")
    } else {
        key.to_string()
    }
}
