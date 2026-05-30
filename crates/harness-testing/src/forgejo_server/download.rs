//! Pinned, checksum-verified Forgejo binary cache under `.cache/forgejo/`.
//!
//! The Forgejo end-to-end harness needs a real server binary. To keep the
//! default test suite hermetic, this module never runs unless an env-gated,
//! `#[ignore]`d test calls [`ensure_binary`]; the cache directory is gitignored
//! (`/.cache/`). The binary is pinned to an exact version and verified against a
//! known SHA-256 after download, so a corrupted or substituted download fails
//! loudly instead of running.
//!
//! Env overrides (for CI, offline machines, or a version bump):
//! - `HARNESS_FORGEJO_BINARY` — absolute path to a pre-downloaded binary; used
//!   as-is, no download and no checksum (the operator vouches for it).
//! - `HARNESS_FORGEJO_URL` — override the download URL (paired with
//!   `HARNESS_FORGEJO_SHA256` to check it; without a sha, the check is skipped).
//! - `HARNESS_FORGEJO_VERSION` — override the pinned version in the default URL.
//! - `HARNESS_FORGEJO_SHA256` — override the expected checksum.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Pinned Forgejo version. Proven by the Phase 0 spike (linux-amd64, SQLite).
pub const FORGEJO_VERSION: &str = "7.0.12";

/// SHA-256 of the pinned `forgejo-7.0.12-linux-amd64` release asset.
pub const FORGEJO_SHA256: &str = "ecd25535250aeb8073fdef1a0c9e92f288de1c0cdde24c95a3b61ead6bc9cf7c";

/// How long the one-shot download is allowed to take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// A failure resolving the Forgejo binary.
#[derive(Debug)]
pub enum DownloadError {
    /// The workspace root could not be located.
    WorkspaceRoot(String),
    /// An override path was given but does not exist.
    MissingOverride(PathBuf),
    /// The HTTP download failed.
    Http(String),
    /// Writing the binary to the cache failed.
    Io(std::io::Error),
    /// The downloaded bytes did not match the expected checksum.
    Checksum { expected: String, actual: String },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::WorkspaceRoot(why) => write!(f, "cannot locate workspace root: {why}"),
            DownloadError::MissingOverride(path) => {
                write!(
                    f,
                    "HARNESS_FORGEJO_BINARY does not exist: {}",
                    path.display()
                )
            }
            DownloadError::Http(why) => write!(f, "forgejo download failed: {why}"),
            DownloadError::Io(err) => write!(f, "forgejo cache io error: {err}"),
            DownloadError::Checksum { expected, actual } => write!(
                f,
                "forgejo binary checksum mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        DownloadError::Io(err)
    }
}

/// Ensures the pinned Forgejo binary exists locally and returns its path.
///
/// Resolution order: `HARNESS_FORGEJO_BINARY` override → cached file (verified
/// present) → download to `.cache/forgejo/` and checksum-verify. The returned
/// path is executable.
pub fn ensure_binary() -> Result<PathBuf, DownloadError> {
    if let Some(path) = env_var("HARNESS_FORGEJO_BINARY") {
        let path = PathBuf::from(path);
        if !path.exists() {
            return Err(DownloadError::MissingOverride(path));
        }
        return Ok(path);
    }

    let version = env_var("HARNESS_FORGEJO_VERSION").unwrap_or_else(|| FORGEJO_VERSION.to_string());
    let expected_sha = env_var("HARNESS_FORGEJO_SHA256");
    let url = env_var("HARNESS_FORGEJO_URL").unwrap_or_else(|| default_url(&version));

    let cache_dir = workspace_root()?.join(".cache").join("forgejo");
    let target = cache_dir.join(format!("forgejo-{version}-linux-amd64"));

    // A present binary is trusted: it was checksum-verified when first written
    // (or supplied via an override URL the operator chose).
    if target.exists() {
        return Ok(target);
    }

    std::fs::create_dir_all(&cache_dir)?;
    let bytes = http_get(&url)?;

    // Verify before publishing the file. Default pin always checks; an override
    // URL checks only when paired with HARNESS_FORGEJO_SHA256.
    let expected = if env_var("HARNESS_FORGEJO_URL").is_some() {
        expected_sha
    } else {
        Some(expected_sha.unwrap_or_else(|| FORGEJO_SHA256.to_string()))
    };
    if let Some(expected) = expected {
        verify_checksum(&bytes, &expected)?;
    }

    write_executable(&target, &bytes)?;
    Ok(target)
}

fn default_url(version: &str) -> String {
    format!(
        "https://codeberg.org/forgejo/forgejo/releases/download/v{version}/forgejo-{version}-linux-amd64"
    )
}

fn http_get(url: &str) -> Result<Vec<u8>, DownloadError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|err| DownloadError::Http(err.to_string()))?;
    let response = client
        .get(url)
        .send()
        .map_err(|err| DownloadError::Http(err.to_string()))?;
    if !response.status().is_success() {
        return Err(DownloadError::Http(format!(
            "GET {url} returned status {}",
            response.status()
        )));
    }
    let mut bytes = Vec::new();
    response
        .take(512 * 1024 * 1024) // generous cap; the asset is ~100 MB
        .read_to_end(&mut bytes)
        .map_err(|err| DownloadError::Http(err.to_string()))?;
    Ok(bytes)
}

fn verify_checksum(bytes: &[u8], expected: &str) -> Result<(), DownloadError> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex_lower(&hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(DownloadError::Checksum {
            expected: expected.to_string(),
            actual,
        })
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn write_executable(target: &Path, bytes: &[u8]) -> Result<(), DownloadError> {
    // Write to a unique temp file in the same dir, then rename, so a concurrent
    // or interrupted download never leaves a half-written binary at `target`.
    let tmp = target.with_extension(format!("part-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    set_executable(&tmp)?;
    std::fs::rename(&tmp, target)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), DownloadError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), DownloadError> {
    Ok(())
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Locates the workspace root by walking up from this crate to the dir that
/// holds the top-level `Cargo.toml` workspace manifest.
fn workspace_root() -> Result<PathBuf, DownloadError> {
    // `CARGO_MANIFEST_DIR` is `<root>/crates/harness-testing` at compile time.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|dir| dir.join("Cargo.toml").exists() && dir.join("crates").is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            DownloadError::WorkspaceRoot(format!(
                "no workspace Cargo.toml above {}",
                manifest_dir.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_accepts_match_and_rejects_mismatch() {
        let data = b"hello";
        // sha256("hello")
        let sha = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(verify_checksum(data, sha).is_ok());
        assert!(matches!(
            verify_checksum(data, "00"),
            Err(DownloadError::Checksum { .. })
        ));
    }

    #[test]
    fn default_url_embeds_version() {
        let url = default_url("7.0.12");
        assert!(url.ends_with("/v7.0.12/forgejo-7.0.12-linux-amd64"));
    }

    #[test]
    fn workspace_root_holds_crates_dir() {
        let root = workspace_root().expect("workspace root resolves");
        assert!(root.join("crates").is_dir());
        assert!(root.join("Cargo.toml").is_file());
    }

    #[test]
    fn hex_lower_pads_bytes() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff]), "000fff");
    }
}
