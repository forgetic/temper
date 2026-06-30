// SPDX-License-Identifier: MPL-2.0

use std::path::{Component, Path};

use toml::Value;

use crate::toml_helpers::join_field;
use crate::{Diagnostic, PathReference};

pub(crate) fn collect_path_references(
    value: &Value,
    field_path: &str,
    base_dir: &Path,
    references: &mut Vec<PathReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        Value::Table(table) => {
            for (key, child) in table {
                let child_path = join_field(field_path, key);
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                if is_path_container_key(&normalized) {
                    collect_path_value(
                        child,
                        &child_path,
                        is_path_map_key(&normalized),
                        base_dir,
                        references,
                        diagnostics,
                    );
                } else if is_path_leaf_key(&normalized) {
                    collect_path_value(
                        child,
                        &child_path,
                        false,
                        base_dir,
                        references,
                        diagnostics,
                    );
                } else {
                    collect_path_references(child, &child_path, base_dir, references, diagnostics);
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_path_references(
                    child,
                    &format!("{field_path}[{index}]"),
                    base_dir,
                    references,
                    diagnostics,
                );
            }
        }
        _ => {}
    }
}

fn collect_path_value(
    value: &Value,
    field_path: &str,
    all_strings_are_paths: bool,
    base_dir: &Path,
    references: &mut Vec<PathReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        Value::String(raw) => {
            validate_path_reference(field_path, raw, base_dir, references, diagnostics);
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_path_value(
                    child,
                    &format!("{field_path}[{index}]"),
                    all_strings_are_paths,
                    base_dir,
                    references,
                    diagnostics,
                );
            }
        }
        Value::Table(table) if all_strings_are_paths => {
            for (key, child) in table {
                collect_path_value(
                    child,
                    &join_field(field_path, key),
                    true,
                    base_dir,
                    references,
                    diagnostics,
                );
            }
        }
        Value::Table(_) => {
            collect_path_references(value, field_path, base_dir, references, diagnostics);
        }
        _ => diagnostics.push(Diagnostic::error(field_path, "must be a local path string")),
    }
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
            | "workflow"
            | "config"
            | "credentials"
            | "readme"
            | "fixture"
            | "manifest"
            | "intent_path"
    ) || key.ends_with("_path")
        || key.ends_with("_file")
        || key.ends_with("_dir")
}
