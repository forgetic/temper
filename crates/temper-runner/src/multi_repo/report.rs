//! Per-tick multi-repository reports, failures, and configuration errors.

use super::repository_set::RepositoryTarget;
use crate::{Progress, WorkerError};
use std::error::Error;
use std::fmt;
use temper_forge::RepositoryId;
use temper_workflow::CommandJournal;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryProgress {
    pub repository: RepositoryTarget,
    pub progress: Progress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryFailure {
    pub repository: RepositoryTarget,
    pub message: String,
}

/// Report for one multi-repo tick, including scan-count diagnostics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MultiRepoTickReport {
    /// Combined progress across successful repositories.
    pub progress: Progress,
    /// Repositories attempted in scan order, including failures.
    pub attempted_repositories: Vec<RepositoryTarget>,
    /// Per-repository successes in attempted scan order.
    pub repositories: Vec<RepositoryProgress>,
    /// Per-repository failures in attempted scan order.
    pub failures: Vec<RepositoryFailure>,
}

impl MultiRepoTickReport {
    pub(super) fn record_attempt(&mut self, repository: &RepositoryTarget) {
        self.attempted_repositories.push(repository.clone());
    }

    pub(super) fn record_success(&mut self, repository: RepositoryTarget, progress: Progress) {
        self.progress.changed |= progress.changed;
        self.progress.actions = self.progress.actions.saturating_add(progress.actions);
        self.repositories.push(RepositoryProgress {
            repository,
            progress,
        });
    }

    pub(super) fn record_failure(&mut self, repository: RepositoryTarget, error: WorkerError) {
        self.failures.push(RepositoryFailure {
            repository,
            message: error.to_string(),
        });
    }

    /// Number of repositories this tick attempted to scan.
    pub fn scanned_repository_count(&self) -> usize {
        self.attempted_repositories.len()
    }

    /// Display paths for repositories this tick attempted to scan.
    pub fn scanned_repository_paths(&self) -> Vec<String> {
        self.attempted_repositories
            .iter()
            .map(RepositoryTarget::display_path)
            .collect()
    }

    pub fn into_worker_result(self) -> Result<Progress, WorkerError> {
        if self.failures.is_empty() {
            Ok(self.progress)
        } else {
            Err(WorkerError::MultiRepo(MultiRepoError {
                progress: self.progress,
                failures: self.failures,
            }))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiRepoError {
    pub progress: Progress,
    pub failures: Vec<RepositoryFailure>,
}

impl fmt::Display for MultiRepoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} repositories failed during multi-repo tick",
            self.failures.len()
        )?;
        for failure in &self.failures {
            write!(
                formatter,
                "; {}: {}",
                failure.repository.display_path(),
                failure.message
            )?;
        }
        Ok(())
    }
}

impl Error for MultiRepoError {}

#[derive(Clone, Copy)]
pub struct RepositoryJournal<'a, J: CommandJournal> {
    pub repository: &'a RepositoryId,
    pub journal: &'a J,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MultiRepoConfigError {
    MissingJournal { repository: RepositoryTarget },
}

impl fmt::Display for MultiRepoConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingJournal { repository } => write!(
                formatter,
                "missing command journal for repository {}",
                repository.display_path()
            ),
        }
    }
}

impl Error for MultiRepoConfigError {}
