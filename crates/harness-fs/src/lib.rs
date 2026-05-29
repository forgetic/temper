//! Filesystem-backed Forge implementation.
//!
//! This crate provides the reference local backend used for deterministic
//! development and tests. The implemented slices support the current user,
//! repositories, repository labels, issues, issue comments, and pull requests;
//! remaining Forge operations return a portable unsupported-operation error
//! until their storage model is added.

mod errors;
mod lists;
mod metadata;
mod operations;
mod record_ids;
mod storage;
mod validation;

use std::path::PathBuf;

/// Local filesystem Forge backend.
#[derive(Clone, Debug)]
pub struct FilesystemForge {
    root: PathBuf,
}
