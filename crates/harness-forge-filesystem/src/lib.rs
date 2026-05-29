//! Filesystem-backed Forge implementation.
//!
//! This crate provides the reference local backend used for deterministic
//! development and tests. The implemented slices support the current user,
//! repositories, repository labels, issues, native dependency links, issue
//! comments, pull requests, pull-request comments, pull-request merges, and CI
//! job listing/lookup.

mod ci_jobs;
mod dependencies;
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
