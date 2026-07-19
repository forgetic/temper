// SPDX-License-Identifier: MPL-2.0

//! Filesystem containment checks for benchmark manifests and fixtures.

use std::fs;

use std::path::{Component, Path, PathBuf};

use temper_protocol_agent::WorkspaceContext;

use super::BenchmarkManifestError;

#[derive(Clone, Copy)]
pub(super) enum InputKind {
    File,
    Directory,
}

pub(super) fn resolve_declared_path(
    root: &Path,
    field: &'static str,
    declared: &Path,
    kind: InputKind,
) -> Result<PathBuf, BenchmarkManifestError> {
    validate_relative_path(field, declared)?;
    reject_symlinked_components(root, field, declared)?;
    let joined = root.join(declared);
    let resolved = fs::canonicalize(&joined).map_err(|source| BenchmarkManifestError::Io {
        operation: "resolve declared input",
        path: joined.clone(),
        source,
    })?;
    if !resolved.starts_with(root) {
        return Err(BenchmarkManifestError::Path {
            field,
            path: declared.to_path_buf(),
            reason: "symlink escapes the manifest directory".to_string(),
        });
    }
    let metadata = fs::metadata(&resolved).map_err(|source| BenchmarkManifestError::Io {
        operation: "inspect declared input",
        path: resolved.clone(),
        source,
    })?;
    let valid_kind = match kind {
        InputKind::File => metadata.is_file(),
        InputKind::Directory => metadata.is_dir(),
    };
    if !valid_kind {
        let expected = match kind {
            InputKind::File => "regular file",
            InputKind::Directory => "directory",
        };
        return Err(BenchmarkManifestError::Path {
            field,
            path: declared.to_path_buf(),
            reason: format!("must name a {expected}"),
        });
    }
    Ok(resolved)
}

pub(crate) fn validate_relative_path(
    field: &'static str,
    path: &Path,
) -> Result<(), BenchmarkManifestError> {
    if path.as_os_str().is_empty() {
        return Err(BenchmarkManifestError::Path {
            field,
            path: path.to_path_buf(),
            reason: "must not be empty".to_string(),
        });
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(BenchmarkManifestError::Path {
                    field,
                    path: path.to_path_buf(),
                    reason: "parent traversal is not allowed".to_string(),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(BenchmarkManifestError::Path {
                    field,
                    path: path.to_path_buf(),
                    reason: "absolute paths are not allowed".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn reject_symlinked_components(
    root: &Path,
    field: &'static str,
    declared: &Path,
) -> Result<(), BenchmarkManifestError> {
    let mut current = root.to_path_buf();
    for component in declared.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(component) => current.push(component),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => unreachable!(),
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(BenchmarkManifestError::Path {
                    field,
                    path: declared.to_path_buf(),
                    reason: "input does not exist".to_string(),
                });
            }
            Err(source) => {
                return Err(BenchmarkManifestError::Io {
                    operation: "inspect declared input",
                    path: current.clone(),
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            let reason = match fs::canonicalize(&current) {
                Ok(target) if !target.starts_with(root) => {
                    "symlink escapes the manifest directory".to_string()
                }
                _ => "symlinked inputs are not allowed".to_string(),
            };
            return Err(BenchmarkManifestError::Path {
                field,
                path: declared.to_path_buf(),
                reason,
            });
        }
    }
    Ok(())
}

pub(super) fn validate_context_repositories(
    fixture: &Path,
    context: &WorkspaceContext,
) -> Result<(), BenchmarkManifestError> {
    if context.repos.is_empty() {
        return Err(BenchmarkManifestError::Invalid(
            "workspace context must contain at least one repository".to_string(),
        ));
    }
    let mut resolved = Vec::with_capacity(context.repos.len());
    for repository in &context.repos {
        let declared = Path::new(&repository.dir);
        validate_relative_path("workspace_context.repos[].dir", declared)?;
        reject_symlinked_components(fixture, "workspace_context.repos[].dir", declared)?;
        let path = fs::canonicalize(fixture.join(declared)).map_err(|source| {
            BenchmarkManifestError::Io {
                operation: "resolve context repository",
                path: fixture.join(declared),
                source,
            }
        })?;
        if !path.starts_with(fixture) {
            return Err(BenchmarkManifestError::Path {
                field: "workspace_context.repos[].dir",
                path: declared.to_path_buf(),
                reason: "repository escapes the fixture directory".to_string(),
            });
        }
        if !path.is_dir() {
            return Err(BenchmarkManifestError::Path {
                field: "workspace_context.repos[].dir",
                path: declared.to_path_buf(),
                reason: "repository must name a fixture directory".to_string(),
            });
        }
        resolved.push((repository.id.as_str(), path));
    }
    for (index, (id, path)) in resolved.iter().enumerate() {
        for (other_id, other_path) in resolved.iter().skip(index + 1) {
            if path == other_path || path.starts_with(other_path) || other_path.starts_with(path) {
                return Err(BenchmarkManifestError::Invalid(format!(
                    "context repositories `{id}` and `{other_id}` overlap"
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_fixture_tree(root: &Path) -> Result<(), BenchmarkManifestError> {
    let mut active_directories = Vec::new();
    validate_fixture_directory(root, root, &mut active_directories)
}

fn validate_fixture_directory(
    root: &Path,
    directory: &Path,
    active_directories: &mut Vec<PathBuf>,
) -> Result<(), BenchmarkManifestError> {
    let canonical = fs::canonicalize(directory).map_err(|source| BenchmarkManifestError::Io {
        operation: "resolve fixture directory",
        path: directory.to_path_buf(),
        source,
    })?;
    if active_directories.contains(&canonical) {
        return Err(BenchmarkManifestError::DirectoryCycle {
            path: directory.to_path_buf(),
        });
    }
    active_directories.push(canonical);

    let mut entries = fs::read_dir(directory)
        .map_err(|source| BenchmarkManifestError::Io {
            operation: "read fixture directory",
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| BenchmarkManifestError::Io {
            operation: "read fixture entry",
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        if entry.file_name() == ".git" {
            return Err(BenchmarkManifestError::UnsafeFixture {
                path,
                reason: "pre-existing Git metadata is not allowed".to_string(),
            });
        }
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| BenchmarkManifestError::Io {
                operation: "inspect fixture entry",
                path: path.clone(),
                source,
            })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(classify_fixture_link(root, &path, active_directories));
        }
        if file_type.is_dir() {
            validate_fixture_directory(root, &path, active_directories)?;
        } else if !file_type.is_file() {
            return Err(BenchmarkManifestError::UnsafeFixture {
                path,
                reason: "only regular files and directories are allowed".to_string(),
            });
        }
    }
    active_directories.pop();
    Ok(())
}

fn classify_fixture_link(
    root: &Path,
    link: &Path,
    active_directories: &[PathBuf],
) -> BenchmarkManifestError {
    let target = match fs::read_link(link) {
        Ok(target) => target,
        Err(error) => {
            return BenchmarkManifestError::UnsafeFixture {
                path: link.to_path_buf(),
                reason: format!("cannot read link: {error}"),
            };
        }
    };
    let parent = link.parent().unwrap_or(root);
    let lexical_target = lexical_normalize(if target.is_absolute() {
        target
    } else {
        parent.join(target)
    });
    if active_directories.contains(&lexical_target) {
        return BenchmarkManifestError::DirectoryCycle {
            path: link.to_path_buf(),
        };
    }
    match fs::canonicalize(link) {
        Ok(target) if !target.starts_with(root) => BenchmarkManifestError::UnsafeFixture {
            path: link.to_path_buf(),
            reason: "symlink escapes the fixture directory".to_string(),
        },
        Ok(target) if active_directories.contains(&target) => {
            BenchmarkManifestError::DirectoryCycle {
                path: link.to_path_buf(),
            }
        }
        Ok(_) => BenchmarkManifestError::UnsafeFixture {
            path: link.to_path_buf(),
            reason: "symlinks are not allowed in benchmark fixtures".to_string(),
        },
        Err(_) if !lexical_target.starts_with(root) => BenchmarkManifestError::UnsafeFixture {
            path: link.to_path_buf(),
            reason: "symlink escapes the fixture directory".to_string(),
        },
        Err(error) => BenchmarkManifestError::UnsafeFixture {
            path: link.to_path_buf(),
            reason: format!("dangling or cyclic symlink: {error}"),
        },
    }
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(component) => normalized.push(component),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
