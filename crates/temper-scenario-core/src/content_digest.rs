// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use toml::Value;

use crate::{CheckReport, PathReference, load_resolved_manifest_toml};

const DIGEST_DOMAIN: &[u8] = b"temper.scenario.resolved-content.v1\0";

#[derive(Debug, Error)]
pub enum ScenarioContentDigestError {
    #[error("mapped scenario has no valid manifest")]
    InvalidManifest,
    #[error("read scenario content {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("unsafe scenario content at {path}: {reason}")]
    Unsafe { path: PathBuf, reason: String },
}

/// Hash the canonical resolved manifest, its referenced fixtures, and all files
/// owned by the mapped scenario directory. Filesystem enumeration order and the
/// checkout's absolute path cannot affect the result.
pub fn scenario_content_digest(report: &CheckReport) -> Result<String, ScenarioContentDigestError> {
    let manifest = report
        .manifest
        .as_ref()
        .ok_or(ScenarioContentDigestError::InvalidManifest)?;
    let manifest_path = report
        .manifest_path
        .as_deref()
        .ok_or(ScenarioContentDigestError::InvalidManifest)?;
    let resolved = load_resolved_manifest_toml(manifest_path)
        .map_err(|_| ScenarioContentDigestError::InvalidManifest)?;

    let mut references = manifest.path_references.iter().collect::<Vec<_>>();
    references.sort_by(|left, right| {
        left.field
            .cmp(&right.field)
            .then(left.value.cmp(&right.value))
            .then(left.resolved_path.cmp(&right.resolved_path))
    });
    let references_by_field = references
        .iter()
        .map(|reference| (reference.field.as_str(), *reference))
        .collect::<BTreeMap<_, _>>();

    let mut digest = Sha256::new();
    digest.update(DIGEST_DOMAIN);
    hash_toml_value(&mut digest, &resolved, "", &references_by_field);
    hash_tree(&mut digest, &report.scenario_path, &report.scenario_path)?;
    for reference in references {
        hash_part(&mut digest, b"reference-field", reference.field.as_bytes());
        hash_part(&mut digest, b"reference-value", reference.value.as_bytes());
        hash_tree(
            &mut digest,
            &reference.resolved_path,
            &reference.resolved_path,
        )?;
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn hash_toml_value(
    digest: &mut Sha256,
    value: &Value,
    field: &str,
    references: &BTreeMap<&str, &PathReference>,
) {
    match value {
        Value::String(value) => {
            let stable = references
                .get(field)
                .map(|reference| reference.value.as_str())
                .unwrap_or(value);
            hash_part(digest, b"string", stable.as_bytes());
        }
        Value::Integer(value) => hash_part(digest, b"integer", value.to_string().as_bytes()),
        Value::Float(value) => hash_part(digest, b"float", value.to_string().as_bytes()),
        Value::Boolean(value) => hash_part(digest, b"boolean", value.to_string().as_bytes()),
        Value::Datetime(value) => hash_part(digest, b"datetime", value.to_string().as_bytes()),
        Value::Array(values) => {
            hash_part(digest, b"array-length", values.len().to_string().as_bytes());
            for (index, value) in values.iter().enumerate() {
                let child = format!("{field}[{index}]");
                hash_toml_value(digest, value, &child, references);
            }
        }
        Value::Table(table) => {
            let mut keys = table.keys().collect::<Vec<_>>();
            keys.sort();
            hash_part(digest, b"table-length", keys.len().to_string().as_bytes());
            for key in keys {
                hash_part(digest, b"key", key.as_bytes());
                let child = if field.is_empty() {
                    key.to_string()
                } else {
                    format!("{field}.{key}")
                };
                hash_toml_value(digest, &table[key], &child, references);
            }
        }
    }
}

fn hash_tree(
    digest: &mut Sha256,
    root: &Path,
    path: &Path,
) -> Result<(), ScenarioContentDigestError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| ScenarioContentDigestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(ScenarioContentDigestError::Unsafe {
            path: path.to_path_buf(),
            reason: "symbolic links are not digestible scenario content".to_string(),
        });
    }
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|source| ScenarioContentDigestError::Read {
                path: path.to_path_buf(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| ScenarioContentDigestError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            hash_tree(digest, root, &entry.path())?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(ScenarioContentDigestError::Unsafe {
            path: path.to_path_buf(),
            reason: "only regular files and directories are supported".to_string(),
        });
    }

    let relative = path.strip_prefix(root).unwrap_or(path);
    hash_part(digest, b"file-path", relative.to_string_lossy().as_bytes());
    let content = fs::read(path).map_err(|source| ScenarioContentDigestError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    hash_part(digest, b"file-content", &content);
    Ok(())
}

fn hash_part(digest: &mut Sha256, tag: &[u8], value: &[u8]) {
    digest.update((tag.len() as u64).to_be_bytes());
    digest.update(tag);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}
