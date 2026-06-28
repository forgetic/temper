//! Cross-process serialization for heavyweight root Forgejo e2e tests.
//!
//! The nextest `e2e` and `e2e-capstones` profiles already assign root-package
//! Forgejo scenarios to the `root-forgejo-e2e` test group (`max-threads = 1`),
//! so normal `cargo dev-test-e2e*` runs queue these topologies before spawning
//! them. This advisory file lock deliberately remains as belt-and-suspenders
//! protection for direct `cargo test -- --ignored` invocations, older/manual
//! runners that do not load `.config/nextest.toml`, or accidental profile
//! bypasses. Holding it is cheap once nextest has serialized the tests, and it
//! still prevents several Forgejo/runner/daemon process trees from contending on
//! shared developer or CI hosts.

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
