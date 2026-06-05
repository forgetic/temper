//! Pinned, checksum-verified Forgejo binary cache under `.cache/forgejo/`.
//!
//! The Forgejo end-to-end fixture needs real server and runner binaries. When
//! an ignored/local fixture path starts a server or runner, it resolves the
//! binary with [`ensure_binary`] / [`ensure_runner_binary`]: explicit
//! `*_BINARY` override → cached `.cache/forgejo/` file → download the pinned
//! release asset, verify its SHA-256, and publish it through a process-safe
//! per-target lock plus an atomic same-directory rename. Default non-ignored
//! tests stay offline because they never start the real Forgejo server or
//! runner.
//!
//! Env overrides (for CI, offline machines, or a version bump). The server
//! binary uses the `TEMPER_FORGEJO_*` namespace; the runner mirrors it under
//! `TEMPER_FORGEJO_RUNNER_*`:
//! - `*_BINARY` — absolute path to a pre-downloaded binary; used as-is, no
//!   download and no checksum (the operator vouches for it).
//! - `*_URL` — override the download URL (paired with `*_SHA256` to check it;
//!   without a sha, the check is skipped).
//! - `*_VERSION` — override the pinned version in the default URL.
//! - `*_SHA256` — override the expected checksum.

use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Pinned Forgejo version. Proven by the Phase 0 spike (linux-amd64, SQLite).
pub const FORGEJO_VERSION: &str = "7.0.12";

/// SHA-256 of the pinned `forgejo-7.0.12-linux-amd64` release asset.
pub const FORGEJO_SHA256: &str = "ecd25535250aeb8073fdef1a0c9e92f288de1c0cdde24c95a3b61ead6bc9cf7c";

/// Pinned `forgejo-runner` version. Proven by the Phase 0b CI spike (host mode).
pub const FORGEJO_RUNNER_VERSION: &str = "3.5.1";

/// SHA-256 of the pinned `forgejo-runner-3.5.1-linux-amd64` release asset.
pub const FORGEJO_RUNNER_SHA256: &str =
    "e2f36aa8149a0e883b5713398aa185c88a827fc0527d5cd2e2b05b88c9ba0b36";

/// How long the one-shot download is allowed to take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// How long to wait for another process to finish publishing the same target.
const LOCK_TIMEOUT: Duration = Duration::from_secs(300);

static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

/// A failure resolving the Forgejo binary.
#[derive(Debug)]
pub enum DownloadError {
    /// The workspace root could not be located.
    WorkspaceRoot(String),
    /// An override path was given but does not exist.
    MissingOverride { variable: String, path: PathBuf },
    /// The HTTP download failed.
    Http(String),
    /// Writing the binary to the cache failed.
    Io(std::io::Error),
    /// Waiting for the per-target cache lock failed.
    Lock(String),
    /// The downloaded bytes did not match the expected checksum.
    Checksum { expected: String, actual: String },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::WorkspaceRoot(why) => write!(f, "cannot locate workspace root: {why}"),
            DownloadError::MissingOverride { variable, path } => {
                write!(f, "{variable} does not exist: {}", path.display())
            }
            DownloadError::Http(why) => write!(f, "forgejo download failed: {why}"),
            DownloadError::Io(err) => write!(f, "forgejo cache io error: {err}"),
            DownloadError::Lock(why) => write!(f, "forgejo cache lock error: {why}"),
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

/// One pinned, env-overridable binary: its env namespace, default pin, and a
/// function turning a (possibly overridden) version into a download URL and
/// cache filename. The `Forgejo` and the `Runner` instances differ only in
/// these fields, so the resolve logic below is shared.
struct Pin {
    /// Env prefix, e.g. `TEMPER_FORGEJO` or `TEMPER_FORGEJO_RUNNER`.
    env_prefix: &'static str,
    /// Default pinned version when `<prefix>_VERSION` is unset.
    default_version: &'static str,
    /// Default expected SHA-256 when `<prefix>_SHA256` is unset.
    default_sha256: &'static str,
    /// Builds the default download URL for a version.
    url: fn(&str) -> String,
    /// Builds the cache filename for a version.
    file_name: fn(&str) -> String,
}

const FORGEJO_PIN: Pin = Pin {
    env_prefix: "TEMPER_FORGEJO",
    default_version: FORGEJO_VERSION,
    default_sha256: FORGEJO_SHA256,
    url: |version| {
        format!(
            "https://codeberg.org/forgejo/forgejo/releases/download/v{version}/forgejo-{version}-linux-amd64"
        )
    },
    file_name: |version| format!("forgejo-{version}-linux-amd64"),
};

const FORGEJO_RUNNER_PIN: Pin = Pin {
    env_prefix: "TEMPER_FORGEJO_RUNNER",
    default_version: FORGEJO_RUNNER_VERSION,
    default_sha256: FORGEJO_RUNNER_SHA256,
    url: |version| {
        format!(
            "https://code.forgejo.org/forgejo/runner/releases/download/v{version}/forgejo-runner-{version}-linux-amd64"
        )
    },
    file_name: |version| format!("forgejo-runner-{version}-linux-amd64"),
};

/// Ensures the pinned Forgejo **server** binary exists locally and returns its
/// path.
///
/// Resolution order: `TEMPER_FORGEJO_BINARY` override → cached file (verified
/// present) → download to `.cache/forgejo/` and checksum-verify. The returned
/// path is executable.
pub fn ensure_binary() -> Result<PathBuf, DownloadError> {
    ensure_pinned(&FORGEJO_PIN)
}

/// Ensures the pinned `forgejo-runner` binary exists locally and returns its
/// path.
///
/// Mirrors [`ensure_binary`] under the `TEMPER_FORGEJO_RUNNER_*` env namespace
/// and shares the same cache dir (`.cache/forgejo/`), download/verify/atomic
/// write logic.
pub fn ensure_runner_binary() -> Result<PathBuf, DownloadError> {
    ensure_pinned(&FORGEJO_RUNNER_PIN)
}

/// Shared resolver for a [`Pin`]: env override path → cached file → verified
/// download.
fn ensure_pinned(pin: &Pin) -> Result<PathBuf, DownloadError> {
    let cache_dir = workspace_root()?.join(".cache").join("forgejo");
    ensure_pinned_with_fetch(pin, &cache_dir, http_get)
}

fn ensure_pinned_with_fetch<F>(
    pin: &Pin,
    cache_dir: &Path,
    fetch: F,
) -> Result<PathBuf, DownloadError>
where
    F: FnOnce(&str) -> Result<Vec<u8>, DownloadError>,
{
    if let Some(path) = env_var(&format!("{}_BINARY", pin.env_prefix)) {
        let path = PathBuf::from(path);
        if !path.exists() {
            return Err(DownloadError::MissingOverride {
                variable: format!("{}_BINARY", pin.env_prefix),
                path,
            });
        }
        return Ok(path);
    }

    let version = env_var(&format!("{}_VERSION", pin.env_prefix))
        .unwrap_or_else(|| pin.default_version.to_string());
    let expected_sha = env_var(&format!("{}_SHA256", pin.env_prefix));
    let url_override = env_var(&format!("{}_URL", pin.env_prefix));
    let url = url_override.clone().unwrap_or_else(|| (pin.url)(&version));
    let target = cache_target(cache_dir, pin, &version);

    // Verify before publishing the file. Default pin always checks; an override
    // URL checks only when paired with `<prefix>_SHA256`.
    let expected = if url_override.is_some() {
        expected_sha
    } else {
        Some(expected_sha.unwrap_or_else(|| pin.default_sha256.to_string()))
    };

    resolve_cached_binary(&target, &url, expected.as_deref(), fetch)
}

fn cache_target(cache_dir: &Path, pin: &Pin, version: &str) -> PathBuf {
    cache_dir.join((pin.file_name)(version))
}

fn resolve_cached_binary<F>(
    target: &Path,
    url: &str,
    expected_sha: Option<&str>,
    fetch: F,
) -> Result<PathBuf, DownloadError>
where
    F: FnOnce(&str) -> Result<Vec<u8>, DownloadError>,
{
    // A present binary is trusted: it was checksum-verified when first written
    // (or supplied via an override URL the operator chose).
    if target.exists() {
        return Ok(target.to_path_buf());
    }

    let cache_dir = target_parent(target)?;
    std::fs::create_dir_all(cache_dir)?;
    let _lock = CacheLock::acquire(&lock_path(target)?)?;

    // Another process may have completed the first-use download while this
    // process waited for the per-target lock.
    if target.exists() {
        return Ok(target.to_path_buf());
    }

    let bytes = fetch(url)?;
    if let Some(expected) = expected_sha {
        verify_checksum(&bytes, expected)?;
    }

    write_executable(target, &bytes)?;
    Ok(target.to_path_buf())
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
    let (tmp, mut file) = create_unique_temp_file(target)?;
    let published = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        set_executable(&tmp)?;

        // Under the per-target lock this should already be false, but keeping
        // the check here makes the publishing primitive safe if a legacy or
        // external publisher raced us to the final path.
        if target.exists() {
            return Ok(false);
        }

        match std::fs::rename(&tmp, target) {
            Ok(()) => Ok(true),
            Err(err) if target.exists() => Ok(false),
            Err(err) => Err(DownloadError::Io(err)),
        }
    })();

    match published {
        Ok(true) => Ok(()),
        Ok(false) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn create_unique_temp_file(target: &Path) -> Result<(PathBuf, File), DownloadError> {
    let parent = target_parent(target)?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy())
        .ok_or_else(|| io_error("cache target has no file name"))?;
    for _ in 0..1_000 {
        let id = NEXT_TMP.fetch_add(1, Ordering::SeqCst);
        let tmp = parent.join(format!(".{name}.part-{}-{id}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(DownloadError::Io(err)),
        }
    }
    Err(DownloadError::Lock(format!(
        "could not allocate a unique temp file next to {}",
        target.display()
    )))
}

fn target_parent(target: &Path) -> Result<&Path, DownloadError> {
    target
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| io_error("cache target has no parent directory"))
}

fn lock_path(target: &Path) -> Result<PathBuf, DownloadError> {
    let parent = target_parent(target)?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy())
        .ok_or_else(|| io_error("cache target has no file name"))?;
    Ok(parent.join(format!(".{name}.lock")))
}

fn io_error(message: &'static str) -> DownloadError {
    DownloadError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}

struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    fn acquire(path: &Path) -> Result<Self, DownloadError> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match std::fs::create_dir(path) {
                Ok(()) => {
                    if let Err(err) =
                        std::fs::write(path.join("owner"), std::process::id().to_string())
                    {
                        let _ = std::fs::remove_dir_all(path);
                        return Err(DownloadError::Io(err));
                    }
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return Err(DownloadError::Lock(format!(
                            "timed out waiting for binary-cache lock {}",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(DownloadError::Io(err)),
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
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

/// Locates the workspace root by walking up from runtime paths to the dir that
/// holds the top-level `Cargo.toml` workspace manifest.
pub(crate) fn workspace_root() -> Result<PathBuf, DownloadError> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_dir() {
        candidates.push(path);
    }
    if let Some(value) = std::env::var_os("CARGO_MANIFEST_DIR") {
        if !value.is_empty() {
            let path = PathBuf::from(value);
            if !candidates.iter().any(|existing| existing == &path) {
                candidates.push(path);
            }
        }
    }

    find_workspace_root(candidates.clone()).ok_or_else(|| {
        let checked = candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        DownloadError::WorkspaceRoot(format!(
            "no workspace Cargo.toml above runtime candidates: {checked}"
        ))
    })
}

fn find_workspace_root(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find_map(|candidate| workspace_root_from(&candidate))
}

fn workspace_root_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| looks_like_temper_workspace_root(dir))
        .map(Path::to_path_buf)
}

fn looks_like_temper_workspace_root(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file() && dir.join("crates").join("temper-forgejo-fixture").is_dir()
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
        let url = (FORGEJO_PIN.url)("7.0.12");
        assert!(url.ends_with("/v7.0.12/forgejo-7.0.12-linux-amd64"));
        assert_eq!(
            (FORGEJO_PIN.file_name)("7.0.12"),
            "forgejo-7.0.12-linux-amd64"
        );
    }

    #[test]
    fn runner_pin_url_and_file_name() {
        let url = (FORGEJO_RUNNER_PIN.url)("3.5.1");
        assert!(
            url.ends_with("/v3.5.1/forgejo-runner-3.5.1-linux-amd64"),
            "unexpected runner url: {url}"
        );
        assert!(url.contains("code.forgejo.org/forgejo/runner/releases"));
        assert_eq!(
            (FORGEJO_RUNNER_PIN.file_name)("3.5.1"),
            "forgejo-runner-3.5.1-linux-amd64"
        );
    }

    #[test]
    fn workspace_root_holds_crates_dir() {
        let root = workspace_root().expect("workspace root resolves");
        assert!(root.join("crates").is_dir());
        assert!(root.join("Cargo.toml").is_file());
    }

    #[test]
    fn workspace_root_search_skips_stale_candidates() {
        let root = synthetic_workspace("stale-candidate");
        let stale = root.join("deleted").join("crates/temper-forgejo-fixture");
        let nested = root.join("crates/temper-forgejo-fixture/src");

        assert_eq!(find_workspace_root([stale, nested]), Some(root.clone()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_binary_cache_publication_downloads_once() {
        const TEST_PAYLOAD: &[u8] = b"tiny forgejo fixture payload\n";
        const TEST_PAYLOAD_SHA256: &str =
            "8ee6a6db0eae82c49e13cef44b63a083001ceb6b8f7178f502ec69c045aa5819";
        const TEST_PIN: Pin = Pin {
            env_prefix: "TEMPER_FORGEJO_FIXTURE_BINARY_CACHE_TEST_NEVER_SET",
            default_version: "1",
            default_sha256: TEST_PAYLOAD_SHA256,
            url: |_| "fixture://payload".to_string(),
            file_name: |_| "forgejo-fixture-test-binary".to_string(),
        };

        let cache_dir = std::env::temp_dir().join(format!(
            "temper-forgejo-binary-cache-test-{}-{}",
            std::process::id(),
            NEXT_TMP.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let target = cache_target(&cache_dir, &TEST_PIN, TEST_PIN.default_version);
        let fetches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let threads = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads));

        let handles = (0..threads)
            .map(|_| {
                let cache_dir = cache_dir.clone();
                let fetches = std::sync::Arc::clone(&fetches);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ensure_pinned_with_fetch(&TEST_PIN, &cache_dir, move |url| {
                        assert_eq!(url, "fixture://payload");
                        fetches.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(25));
                        Ok(TEST_PAYLOAD.to_vec())
                    })
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            let path = handle.join().expect("publisher thread panicked").unwrap();
            assert_eq!(path, target);
        }

        assert_eq!(std::fs::read(&target).unwrap(), TEST_PAYLOAD);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        let leftovers = std::fs::read_dir(&cache_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(leftovers, vec!["forgejo-fixture-test-binary"]);
        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    #[test]
    fn hex_lower_pads_bytes() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff]), "000fff");
    }

    fn synthetic_workspace(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "temper-forgejo-fixture-{label}-{}-{}",
            std::process::id(),
            NEXT_TMP.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/temper-forgejo-fixture/src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), b"[workspace]\n").unwrap();
        root
    }
}
