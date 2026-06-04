//! Cached Forgejo data-tree states for ignored local tests.
//!
//! A test describes the Forgejo state it needs with JSON, and provides an
//! initializer closure for the first cache miss. The fixture boots a fresh
//! Forgejo once, runs the initializer, shuts the server down cleanly, and
//! publishes the resulting data tree under `.cache/forgejo/states/<hash>/` with
//! a per-key process-safe lock plus an atomic directory rename. Later callers
//! validate the ready marker, metadata, and tree before copying that tree to a
//! unique `/tmp` runtime directory and starting `forgejo web` against it.
//! Runtime servers still die by SIGKILL through `ForgejoServer`'s `Drop` guard.

use crate::{download, unique_data_dir, ForgejoServer, ServerError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CACHE_FORMAT_VERSION: u32 = 1;
const READY_FILE: &str = "READY";
const TREE_DIR: &str = "tree";
const METADATA_FILE: &str = "metadata.json";
const DESCRIPTION_FILE: &str = "description.json";
const LOCK_TIMEOUT: Duration = Duration::from_secs(300);

static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

/// A stable JSON description of the initial Forgejo state a test needs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ForgejoState {
    description: serde_json::Value,
}

impl ForgejoState {
    /// Builds a state description from any serializable value.
    pub fn new(description: impl Serialize) -> Result<Self, ServerError> {
        Ok(Self {
            description: serde_json::to_value(description)?,
        })
    }

    /// A named empty state, useful for lifecycle smoke tests.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            description: serde_json::json!({
                "kind": "empty",
                "name": name.into(),
            }),
        }
    }

    /// Returns the JSON value that participates in cache-key hashing.
    pub fn description(&self) -> &serde_json::Value {
        &self.description
    }

    /// Stable hash used as the cache directory name.
    pub fn cache_key(&self) -> Result<String, ServerError> {
        let input = CacheKeyInput {
            cache_format: CACHE_FORMAT_VERSION,
            forgejo_binary: forgejo_binary_fingerprint(),
            description: &self.description,
        };
        let bytes = serde_json::to_vec(&input)?;
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Ok(hex_lower(&hasher.finalize()))
    }
}

/// A running Forgejo server restored from a cached initial state, plus the
/// metadata returned by the initializer that created that state.
pub struct CachedForgejo<T> {
    /// Running server backed by a per-test copy of the cached tree.
    pub server: ForgejoServer,
    /// Initializer-produced metadata (tokens, repository ids, etc.).
    pub metadata: T,
    /// Cache key for diagnostics.
    pub cache_key: String,
    /// `true` when the tree already existed before this call.
    pub cache_hit: bool,
}

#[derive(Serialize)]
struct CacheKeyInput<'a> {
    cache_format: u32,
    forgejo_binary: String,
    description: &'a serde_json::Value,
}

impl ForgejoServer {
    /// Starts Forgejo from a JSON-declared cached state.
    ///
    /// On the first cache miss for `state`, `initialize` runs against a fresh
    /// server and returns metadata that is stored next to the cached tree. On
    /// later calls the initializer is skipped and the metadata is read back from
    /// `.cache`. In all cases the returned server is a new process against a
    /// fresh `/tmp` copy of the cached tree.
    pub fn start_with_state<T, E, F>(
        state: &ForgejoState,
        initialize: F,
    ) -> Result<CachedForgejo<T>, ServerError>
    where
        T: DeserializeOwned + Serialize,
        E: std::fmt::Display,
        F: FnOnce(&ForgejoServer) -> Result<T, E>,
    {
        start_with_state(state, initialize)
    }
}

fn start_with_state<T, E, F>(
    state: &ForgejoState,
    initialize: F,
) -> Result<CachedForgejo<T>, ServerError>
where
    T: DeserializeOwned + Serialize,
    E: std::fmt::Display,
    F: FnOnce(&ForgejoServer) -> Result<T, E>,
{
    let key = state.cache_key()?;
    let paths = StatePaths::new(&key)?;
    let cache_hit = ensure_cached(&paths, state, initialize)?;
    let metadata = read_metadata(&paths.metadata)?;
    let data_dir = copy_cached_tree_to_tmp(&paths.tree)?;
    let server = ForgejoServer::start_from_prepared_dir(data_dir)?;
    Ok(CachedForgejo {
        server,
        metadata,
        cache_key: key,
        cache_hit,
    })
}

fn ensure_cached<T, E, F>(
    paths: &StatePaths,
    state: &ForgejoState,
    initialize: F,
) -> Result<bool, ServerError>
where
    T: DeserializeOwned + Serialize,
    E: std::fmt::Display,
    F: FnOnce(&ForgejoServer) -> Result<T, E>,
{
    if cache_ready::<T>(paths)? {
        return Ok(true);
    }
    std::fs::create_dir_all(&paths.parent)?;
    let _lock = CacheLock::acquire(&paths.lock)?;
    if cache_ready::<T>(paths)? {
        return Ok(true);
    }
    build_cache(paths, state, initialize)?;
    Ok(false)
}

fn cache_ready<T: DeserializeOwned>(paths: &StatePaths) -> Result<bool, ServerError> {
    if std::fs::read(&paths.ready).is_err() {
        return Ok(false);
    }
    if !paths.tree.is_dir() || std::fs::read_dir(&paths.tree).is_err() {
        return Ok(false);
    }
    let metadata = match std::fs::read(&paths.metadata) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(false),
    };
    Ok(serde_json::from_slice::<T>(&metadata).is_ok())
}

fn build_cache<T, E, F>(
    paths: &StatePaths,
    state: &ForgejoState,
    initialize: F,
) -> Result<(), ServerError>
where
    T: Serialize,
    E: std::fmt::Display,
    F: FnOnce(&ForgejoServer) -> Result<T, E>,
{
    let mut server = ForgejoServer::start()?;
    let metadata = initialize(&server).map_err(|err| ServerError::Initialize(err.to_string()))?;
    server.stop_cleanly()?;

    let tmp = paths.parent.join(format!(
        ".{}.tmp-{}-{}",
        paths.key,
        std::process::id(),
        NEXT_TMP.fetch_add(1, Ordering::SeqCst)
    ));
    remove_path_if_exists(&tmp)?;
    std::fs::create_dir_all(&tmp)?;
    copy_dir(server.data_dir(), &tmp.join(TREE_DIR))?;
    write_json(&tmp.join(METADATA_FILE), &metadata)?;
    write_json(&tmp.join(DESCRIPTION_FILE), state.description())?;
    std::fs::write(tmp.join(READY_FILE), b"ready\n")?;

    remove_path_if_exists(&paths.root)?;
    std::fs::rename(&tmp, &paths.root)?;
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<(), ServerError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir_all(path)?;
            Ok(())
        }
        Ok(_) => {
            std::fs::remove_file(path)?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ServerError::Io(err)),
    }
}

fn copy_cached_tree_to_tmp(cached_tree: &Path) -> Result<PathBuf, ServerError> {
    let data_dir = unique_data_dir();
    let _ = std::fs::remove_dir_all(&data_dir);
    copy_dir(cached_tree, &data_dir)?;
    Ok(data_dir)
}

fn read_metadata<T: DeserializeOwned>(path: &Path) -> Result<T, ServerError> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), ServerError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    std::fs::write(path, bytes)?;
    Ok(())
}

struct StatePaths {
    key: String,
    parent: PathBuf,
    root: PathBuf,
    tree: PathBuf,
    metadata: PathBuf,
    ready: PathBuf,
    lock: PathBuf,
}

impl StatePaths {
    fn new(key: &str) -> Result<Self, ServerError> {
        let parent = download::workspace_root()?
            .join(".cache")
            .join("forgejo")
            .join("states");
        let root = parent.join(key);
        Ok(Self {
            key: key.to_string(),
            parent,
            tree: root.join(TREE_DIR),
            metadata: root.join(METADATA_FILE),
            ready: root.join(READY_FILE),
            lock: root.with_extension("lock"),
            root,
        })
    }
}

struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    fn acquire(path: &Path) -> Result<Self, ServerError> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match std::fs::create_dir(path) {
                Ok(()) => {
                    if let Err(err) =
                        std::fs::write(path.join("owner"), std::process::id().to_string())
                    {
                        let _ = std::fs::remove_dir_all(path);
                        return Err(ServerError::Io(err));
                    }
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return Err(ServerError::Cache(format!(
                            "timed out waiting for state-cache lock {}",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(err) => return Err(ServerError::Io(err)),
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), ServerError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn forgejo_binary_fingerprint() -> String {
    if let Some(path) = env_var("TEMPER_FORGEJO_BINARY") {
        format!("binary:{path}")
    } else {
        format!(
            "version:{}",
            env_var("TEMPER_FORGEJO_VERSION").unwrap_or_else(|| download::FORGEJO_VERSION.into())
        )
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_description_sensitive() {
        let a = ForgejoState::new(serde_json::json!({"repos": ["one"]})).unwrap();
        let b = ForgejoState::new(serde_json::json!({"repos": ["one"]})).unwrap();
        let c = ForgejoState::new(serde_json::json!({"repos": ["two"]})).unwrap();
        assert_eq!(a.cache_key().unwrap(), b.cache_key().unwrap());
        assert_ne!(a.cache_key().unwrap(), c.cache_key().unwrap());
    }

    #[test]
    fn cache_ready_requires_marker_tree_and_metadata() {
        let parent = std::env::temp_dir().join(format!(
            "temper-forgejo-state-ready-test-{}-{}",
            std::process::id(),
            NEXT_TMP.fetch_add(1, Ordering::SeqCst)
        ));
        let root = parent.join("state");
        let paths = StatePaths {
            key: "state".to_string(),
            parent: parent.clone(),
            tree: root.join(TREE_DIR),
            metadata: root.join(METADATA_FILE),
            ready: root.join(READY_FILE),
            lock: root.with_extension("lock"),
            root,
        };
        let _ = std::fs::remove_dir_all(&parent);

        assert!(!cache_ready::<serde_json::Value>(&paths).unwrap());
        std::fs::create_dir_all(&paths.tree).unwrap();
        std::fs::write(&paths.metadata, b"{\"token\":\"abc\"}").unwrap();
        std::fs::write(&paths.ready, b"ready\n").unwrap();
        assert!(cache_ready::<serde_json::Value>(&paths).unwrap());

        std::fs::remove_file(&paths.metadata).unwrap();
        assert!(!cache_ready::<serde_json::Value>(&paths).unwrap());
        std::fs::write(&paths.metadata, b"not json").unwrap();
        assert!(!cache_ready::<serde_json::Value>(&paths).unwrap());
        std::fs::write(&paths.metadata, b"{}").unwrap();
        std::fs::remove_dir_all(&paths.tree).unwrap();
        assert!(!cache_ready::<serde_json::Value>(&paths).unwrap());
        std::fs::create_dir_all(&paths.tree).unwrap();
        std::fs::remove_file(&paths.ready).unwrap();
        assert!(!cache_ready::<serde_json::Value>(&paths).unwrap());

        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn copy_dir_recurses() {
        let root = std::env::temp_dir().join(format!(
            "temper-forgejo-copy-test-{}-{}",
            std::process::id(),
            NEXT_TMP.fetch_add(1, Ordering::SeqCst)
        ));
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("a/b")).unwrap();
        std::fs::write(src.join("a/b/file.txt"), "hello").unwrap();
        copy_dir(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a/b/file.txt")).unwrap(),
            "hello"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
