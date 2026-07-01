// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use toml::Value;

use crate::sourced::{SourcedValue, overlay};
use crate::{Diagnostic, MANIFEST_FILE_NAMES};

const EXTENDS_FIELD: &str = "fixtures.extends";

pub(crate) fn resolve_manifest_file(path: &Path) -> Result<SourcedValue, Vec<Diagnostic>> {
    let mut stack = Vec::new();
    resolve_manifest_file_inner(path, &mut stack)
}

pub(crate) fn resolve_manifest_value(
    value: Value,
    base_dir: &Path,
    manifest_path: Option<&Path>,
) -> Result<SourcedValue, Vec<Diagnostic>> {
    let mut stack = Vec::new();
    if let Some(manifest_path) = manifest_path {
        stack.push(canonical_or_absolute(manifest_path));
    }
    resolve_manifest_value_inner(value, base_dir, &mut stack)
}

fn resolve_manifest_file_inner(
    path: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<SourcedValue, Vec<Diagnostic>> {
    let identity = canonical_or_absolute(path);
    if stack.iter().any(|ancestor| ancestor == &identity) {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            format!(
                "fixture inheritance cycle detected at {}",
                identity.display()
            ),
        )]);
    }
    stack.push(identity.clone());

    let result = (|| {
        let source = fs::read_to_string(&identity).map_err(|error| {
            vec![Diagnostic::error(
                EXTENDS_FIELD,
                format!(
                    "failed to read inherited manifest {}: {error}",
                    identity.display()
                ),
            )]
        })?;
        let value = source.parse::<Value>().map_err(|error| {
            vec![Diagnostic::error(
                EXTENDS_FIELD,
                format!(
                    "failed to parse inherited manifest {}: {error}",
                    identity.display()
                ),
            )]
        })?;
        let base_dir = identity.parent().unwrap_or_else(|| Path::new("."));
        resolve_manifest_value_inner(value, base_dir, stack)
    })();

    stack.pop();
    result
}

fn resolve_manifest_value_inner(
    value: Value,
    base_dir: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<SourcedValue, Vec<Diagnostic>> {
    let inheritance = fixture_extends(&value)?;
    let child = SourcedValue::from_value(value, base_dir.to_path_buf());
    let Some(raw_base) = inheritance else {
        return Ok(child);
    };

    let base_path = resolve_extends_path(&raw_base, base_dir)?;
    let base_manifest = resolve_extends_manifest(&base_path)?;
    let parent = resolve_manifest_file_inner(&base_manifest, stack)?;
    Ok(overlay(parent, child))
}

fn fixture_extends(value: &Value) -> Result<Option<String>, Vec<Diagnostic>> {
    let Some(root) = value.as_table() else {
        return Ok(None);
    };
    let Some(fixtures) = root.get("fixtures") else {
        return Ok(None);
    };
    let Some(fixtures) = fixtures.as_table() else {
        return Err(vec![Diagnostic::error(
            "fixtures",
            "must be a table when declaring fixture inheritance",
        )]);
    };
    let Some(extends) = fixtures.get("extends") else {
        return Ok(None);
    };
    let Some(raw) = extends.as_str() else {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            "must be a local relative path string",
        )]);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            "path must not be empty",
        )]);
    }
    Ok(Some(trimmed.to_string()))
}

fn resolve_extends_path(raw: &str, base_dir: &Path) -> Result<PathBuf, Vec<Diagnostic>> {
    validate_local_extends_path(raw)?;
    let relative = Path::new(raw);
    let mut candidates = vec![base_dir.join(relative)];
    for root in candidate_workspace_roots(base_dir) {
        push_unique(&mut candidates, root.join(relative));
    }

    let Some(candidate) = candidates.iter().find(|candidate| candidate.exists()) else {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            format!("fixture inheritance base does not exist: {raw}"),
        )]);
    };

    let canonical = fs::canonicalize(candidate).map_err(|error| {
        vec![Diagnostic::error(
            EXTENDS_FIELD,
            format!(
                "failed to canonicalize fixture inheritance base {}: {error}",
                candidate.display()
            ),
        )]
    })?;

    let allowed_roots = allowed_roots(base_dir);
    if !allowed_roots.iter().any(|root| canonical.starts_with(root)) {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            format!("fixture inheritance base escapes allowed scenario/workspace roots: {raw}"),
        )]);
    }

    Ok(canonical)
}

fn validate_local_extends_path(raw: &str) -> Result<(), Vec<Diagnostic>> {
    if raw.contains("://") {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            "must be a local relative path, not a URL",
        )]);
    }
    let relative = Path::new(raw);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            "must be a local relative path without `..` components",
        )]);
    }
    Ok(())
}

fn resolve_extends_manifest(path: &Path) -> Result<PathBuf, Vec<Diagnostic>> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        if let Some(manifest) = manifest_in_dir(path) {
            return Ok(manifest);
        }
        return Err(vec![Diagnostic::error(
            EXTENDS_FIELD,
            format!(
                "fixture inheritance base has no scenario manifest: {} (expected one of: {})",
                path.display(),
                MANIFEST_FILE_NAMES.join(", ")
            ),
        )]);
    }
    Err(vec![Diagnostic::error(
        EXTENDS_FIELD,
        format!(
            "fixture inheritance base is not readable: {}",
            path.display()
        ),
    )])
}

fn manifest_in_dir(dir: &Path) -> Option<PathBuf> {
    MANIFEST_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

fn allowed_roots(base_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique(&mut roots, canonical_or_absolute(base_dir));
    for root in candidate_workspace_roots(base_dir) {
        push_unique(&mut roots, canonical_or_absolute(&root));
    }
    roots
}

fn candidate_workspace_roots(base_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = find_workspace_root(base_dir) {
        push_unique(&mut roots, root);
    }
    if let Ok(current_dir) = env::current_dir() {
        if let Some(root) = find_workspace_root(&current_dir) {
            push_unique(&mut roots, root);
        }
    }
    push_unique(
        &mut roots,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    );
    roots
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    loop {
        if current.join("Cargo.toml").is_file() && current.join("scenarios").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}
