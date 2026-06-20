//! Cross-process serialization for heavyweight root Forgejo e2e tests.
//!
//! `cargo dev-test-full` runs ignored tests in parallel via nextest. The root
//! Forgejo scenarios each spawn real Forgejo/runner/daemon/worker processes;
//! running several of those topologies at once can starve the polling loops long
//! enough to trip convergence timeouts even though each scenario is stable in
//! isolation. Hold this advisory file lock for the lifetime of one scenario so
//! the heavyweight root e2es run one at a time while the rest of the workspace
//! test suite remains parallel.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use fs2::FileExt;

pub struct E2eLock {
    _file: File,
}

pub fn acquire() -> E2eLock {
    let path = lock_path();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap_or_else(|error| panic!("open e2e lock {}: {error}", path.display()));
    file.lock_exclusive()
        .unwrap_or_else(|error| panic!("acquire e2e lock {}: {error}", path.display()));
    E2eLock { _file: file }
}

fn lock_path() -> PathBuf {
    std::env::temp_dir().join("temper-root-forgejo-e2e.lock")
}
