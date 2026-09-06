//! Bounded checkout fingerprint, including untracked and excluded scoped files.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn fingerprint(root: &Path, input: &Value) -> Result<String, &'static str> {
    let mut paths = BTreeSet::new();
    for path in input["paths"].as_array().into_iter().flatten() {
        paths.insert(root.join(path.as_str().ok_or("invalid source path")?));
    }
    for scope in input["scopes"].as_array().into_iter().flatten() {
        collect(
            root.join(scope.as_str().ok_or("invalid source scope")?),
            &mut paths,
        )?;
    }
    if paths.len() > MAX_FILES {
        return Err(
            "scope freshness unavailable: exceeds 512 entries; narrow scope or limit claim",
        );
    }
    let mut digest = Sha256::new();
    digest.update(super::super::scope::current_git_head(root).unwrap_or_default());
    let mut total = 0;
    for path in paths {
        digest.update(
            path.strip_prefix(root)
                .map_err(|_| "source outside checkout")?
                .to_string_lossy()
                .as_bytes(),
        );
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err("source freshness unavailable: cited path or scope does not exist");
            }
            Err(_) => return Err("source freshness unavailable"),
        };
        if metadata.file_type().is_symlink() {
            digest.update(b"symlink");
            digest.update(
                std::fs::read_link(&path)
                    .map_err(|_| "source freshness unavailable")?
                    .to_string_lossy()
                    .as_bytes(),
            );
            // Do not fingerprint data beyond an unresolved alias. Explicit path
            // validation already rejects escapes; scope aliases limit claims.
            return Err(
                "scope freshness unavailable: symbolic link; narrow to canonical source paths",
            );
        }
        if metadata.is_dir() {
            digest.update(b"directory");
            continue;
        }
        if !metadata.is_file() {
            return Err("scope freshness unavailable: nonregular file");
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "source freshness unavailable")?;
        if !canonical.starts_with(root) {
            return Err("source outside checkout");
        }
        let mut bytes = Vec::new();
        std::fs::File::open(canonical)
            .map_err(|_| "source freshness unavailable")?
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "source freshness unavailable")?;
        total += bytes.len();
        if bytes.len() > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
            return Err(
                "scope freshness unavailable: source bytes exceed bound; narrow scope or limit claim",
            );
        }
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn collect(path: PathBuf, paths: &mut BTreeSet<PathBuf>) -> Result<(), &'static str> {
    paths.insert(path.clone());
    if paths.len() > MAX_FILES {
        return Err(
            "scope freshness unavailable: exceeds 512 entries; narrow scope or limit claim",
        );
    }
    let Ok(metadata) = path.symlink_metadata() else {
        return Ok(());
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    for entry in std::fs::read_dir(path).map_err(|_| "scope freshness unavailable")? {
        let entry = entry.map_err(|_| "scope freshness unavailable")?;
        // Git administrative state is not repository source. HEAD is included
        // separately; tracked/untracked/ignored source all participate below.
        if entry.file_name() == ".git" {
            continue;
        }
        collect(entry.path(), paths)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn missing_cited_source_or_scope_cannot_receive_a_fresh_identity() {
        let root = tempfile::tempdir().unwrap();
        for input in [
            json!({"paths":["missing.rs"]}),
            json!({"scopes":["missing"]}),
        ] {
            assert!(
                fingerprint(root.path(), &input)
                    .unwrap_err()
                    .contains("unavailable")
            );
        }
    }

    #[test]
    fn overlapping_explicit_directory_does_not_skip_scoped_contents() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("src");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("file.rs");
        std::fs::write(&path, "one").unwrap();
        let input = json!({"paths":["src"],"scopes":["src"]});
        let before = fingerprint(root.path(), &input).unwrap();
        std::fs::write(path, "two").unwrap();
        assert_ne!(before, fingerprint(root.path(), &input).unwrap());
    }

    #[test]
    fn scope_only_identity_detects_untracked_added_edited_and_deleted_source() {
        let root = tempfile::tempdir().unwrap();
        let scope = json!({"scopes":["."]});
        let before = fingerprint(root.path(), &scope).unwrap();
        std::fs::write(root.path().join("file.rs"), "one").unwrap();
        let added = fingerprint(root.path(), &scope).unwrap();
        assert_ne!(before, added);
        std::fs::write(root.path().join("file.rs"), "two").unwrap();
        assert_ne!(added, fingerprint(root.path(), &scope).unwrap());
        std::fs::remove_file(root.path().join("file.rs")).unwrap();
        assert_eq!(before, fingerprint(root.path(), &scope).unwrap());
    }
}
