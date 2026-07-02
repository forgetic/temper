// SPDX-License-Identifier: MPL-2.0

use std::path::{Component, Path};

use toml::Value;

use crate::sourced::{SourcedValue, SourcedValueKind};
use crate::toml_helpers::join_field;
use crate::{Diagnostic, PathReference};

pub(crate) fn collect_path_references(
    value: &SourcedValue,
    field_path: &str,
    references: &mut Vec<PathReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &value.kind {
        SourcedValueKind::Table(table) => {
            for (key, child) in table {
                let child_path = join_field(field_path, key);
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                if is_path_container_key(&normalized) {
                    collect_path_value(
                        child,
                        &child_path,
                        is_path_map_key(&normalized),
                        references,
                        diagnostics,
                    );
                } else if is_path_leaf_key(&normalized) {
                    collect_path_value(child, &child_path, false, references, diagnostics);
                } else {
                    collect_path_references(child, &child_path, references, diagnostics);
                }
            }
        }
        SourcedValueKind::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_path_references(
                    child,
                    &format!("{field_path}[{index}]"),
                    references,
                    diagnostics,
                );
            }
        }
        _ => {}
    }
}

pub(crate) fn absolutize_path_references(value: &SourcedValue) -> Value {
    absolutize_node(value, "")
}

fn collect_path_value(
    value: &SourcedValue,
    field_path: &str,
    all_strings_are_paths: bool,
    references: &mut Vec<PathReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &value.kind {
        SourcedValueKind::String(raw) => {
            validate_path_reference(field_path, raw, value.origin_dir(), references, diagnostics);
        }
        SourcedValueKind::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_path_value(
                    child,
                    &format!("{field_path}[{index}]"),
                    all_strings_are_paths,
                    references,
                    diagnostics,
                );
            }
        }
        SourcedValueKind::Table(table) if all_strings_are_paths => {
            for (key, child) in table {
                collect_path_value(
                    child,
                    &join_field(field_path, key),
                    true,
                    references,
                    diagnostics,
                );
            }
        }
        SourcedValueKind::Table(_) => {
            collect_path_references(value, field_path, references, diagnostics);
        }
        _ => diagnostics.push(Diagnostic::error(field_path, "must be a local path string")),
    }
}

fn absolutize_node(value: &SourcedValue, field_path: &str) -> Value {
    match &value.kind {
        SourcedValueKind::Table(table) => Value::Table(
            table
                .iter()
                .map(|(key, child)| {
                    let child_path = join_field(field_path, key);
                    let normalized = key.replace('-', "_").to_ascii_lowercase();
                    let child_value = if is_path_container_key(&normalized) {
                        absolutize_path_value(child, &child_path, is_path_map_key(&normalized))
                    } else if is_path_leaf_key(&normalized) {
                        absolutize_path_value(child, &child_path, false)
                    } else {
                        absolutize_node(child, &child_path)
                    };
                    (key.clone(), child_value)
                })
                .collect(),
        ),
        SourcedValueKind::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(index, child)| absolutize_node(child, &format!("{field_path}[{index}]")))
                .collect(),
        ),
        _ => value.to_value(),
    }
}

fn absolutize_path_value(
    value: &SourcedValue,
    field_path: &str,
    all_strings_are_paths: bool,
) -> Value {
    match &value.kind {
        SourcedValueKind::String(raw) => absolutize_path_string(raw, value.origin_dir())
            .map(Value::String)
            .unwrap_or_else(|| value.to_value()),
        SourcedValueKind::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    absolutize_path_value(
                        child,
                        &format!("{field_path}[{index}]"),
                        all_strings_are_paths,
                    )
                })
                .collect(),
        ),
        SourcedValueKind::Table(table) if all_strings_are_paths => Value::Table(
            table
                .iter()
                .map(|(key, child)| {
                    (
                        key.clone(),
                        absolutize_path_value(child, &join_field(field_path, key), true),
                    )
                })
                .collect(),
        ),
        SourcedValueKind::Table(_) => absolutize_node(value, field_path),
        _ => value.to_value(),
    }
}

fn absolutize_path_string(raw: &str, base_dir: &Path) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.contains("://") {
        return None;
    }
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(base_dir.join(relative).display().to_string())
}

fn validate_path_reference(
    field_path: &str,
    raw: &str,
    base_dir: &Path,
    references: &mut Vec<PathReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let value = raw.trim();
    if value.is_empty() {
        diagnostics.push(Diagnostic::error(field_path, "path must not be empty"));
        return;
    }
    if value.contains("://") {
        diagnostics.push(Diagnostic::error(
            field_path,
            "must be a local relative path, not a URL",
        ));
        return;
    }

    let relative = Path::new(value);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        diagnostics.push(Diagnostic::error(
            field_path,
            "must be a local relative path without `..` components",
        ));
        return;
    }

    let resolved_path = base_dir.join(relative);
    if !resolved_path.exists() {
        diagnostics.push(Diagnostic::error(
            field_path,
            format!("referenced path does not exist: {value}"),
        ));
        return;
    }

    references.push(PathReference {
        field: field_path.to_string(),
        value: value.to_string(),
        resolved_path,
    });
}

fn is_path_map_key(key: &str) -> bool {
    matches!(key, "paths" | "dirs" | "directories")
        || key.ends_with("_paths")
        || key.ends_with("_dirs")
}

fn is_path_container_key(key: &str) -> bool {
    is_path_map_key(key)
        || matches!(
            key,
            "files"
                | "required_files"
                | "fixtures"
                | "fixture_files"
                | "inputs"
                | "artifacts"
                | "outputs"
                | "resources"
        )
        || key.ends_with("_files")
}

fn is_path_leaf_key(key: &str) -> bool {
    matches!(
        key,
        "path"
            | "file"
            | "dir"
            | "directory"
            | "body"
            | "workflow"
            | "config"
            | "credentials"
            | "readme"
            | "fixture"
            | "manifest"
            | "intent_path"
            | "ci_source"
            | "command"
            | "cwd"
    ) || key.ends_with("_path")
        || key.ends_with("_file")
        || key.ends_with("_dir")
}
