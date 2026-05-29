//! Filesystem-backed Forge implementation.
//!
//! This crate will provide the reference local backend used for deterministic
//! development and tests. The initial scaffold defines the storage root and
//! layout bootstrap; trait implementation will be added after the core contract
//! settles.

use harness_forge::ForgeError;
use std::path::{Path, PathBuf};

/// Local filesystem Forge backend.
#[derive(Clone, Debug)]
pub struct FilesystemForge {
    root: PathBuf,
}

impl FilesystemForge {
    /// Creates a backend rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the filesystem root used by this backend.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the directory used to store repository records.
    pub fn repositories_dir(&self) -> PathBuf {
        self.root.join("repositories")
    }

    /// Creates the backend directory layout if needed.
    pub fn ensure_layout(&self) -> Result<(), ForgeError> {
        std::fs::create_dir_all(self.repositories_dir())
            .map_err(|error| ForgeError::Backend(error.to_string()))
    }
}
