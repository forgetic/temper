//! Filesystem-backed Forge implementation.
//!
//! This crate provides the reference local backend used for deterministic
//! development and tests. The implemented slices support the current user,
//! handle-local identity overrides, repositories, repository labels, issues,
//! native dependency links, issue comments, pull requests, pull-request comments
//! and reviews, pull-request merges, and CI job listing/lookup.

mod ci_jobs;
mod dependencies;
mod errors;
mod handle;
mod hint;
mod lists;
mod locking;
mod merge;
mod metadata;
mod operations;
mod record_ids;
mod reviews;
mod storage;
mod validation;

use harness_forge::User;
use std::path::PathBuf;

/// Local filesystem Forge backend.
#[derive(Clone, Debug)]
pub struct FilesystemForge {
    root: PathBuf,
    current_user: Option<User>,
}
