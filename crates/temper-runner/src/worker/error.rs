//! Worker tick error type.

use crate::agent::AgentError;
use crate::scan::ScanError;
use crate::signal::CiError;
use std::error::Error;
use std::fmt;
use temper_forge_model::ForgeError;
use temper_workflow::{ApplyError, ExecutionError, ReconcileError};

/// Errors that can stop a worker tick.
#[derive(Debug)]
pub enum WorkerError {
    /// Queue scanning failed.
    Scan(ScanError),
    /// Workflow execution failed outside an agent boundary.
    Execution(ExecutionError),
    /// A direct Forge operation failed outside an agent boundary.
    Forge(ForgeError),
    /// Reconciliation could not load Forge or journal state.
    Reconcile(ReconcileError),
    /// Applying a reconciliation report failed.
    Apply(ApplyError),
    /// The worker's agent failed while servicing work.
    Agent(AgentError),
    /// A fake outside-world CI producer failed.
    Ci(CiError),
    /// One or more repositories failed in a multi-repo wrapper tick.
    MultiRepo(crate::multi_repo::MultiRepoError),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Scan(error) => write!(formatter, "worker scan failed: {error}"),
            WorkerError::Execution(error) => write!(formatter, "worker execution failed: {error}"),
            WorkerError::Forge(error) => {
                write!(formatter, "worker forge operation failed: {error}")
            }
            WorkerError::Reconcile(error) => write!(formatter, "worker reconcile failed: {error}"),
            WorkerError::Apply(error) => write!(formatter, "worker recovery apply failed: {error}"),
            WorkerError::Agent(error) => write!(formatter, "worker agent failed: {error}"),
            WorkerError::Ci(error) => write!(formatter, "worker CI producer failed: {error}"),
            WorkerError::MultiRepo(error) => write!(formatter, "multi-repo worker failed: {error}"),
        }
    }
}

impl Error for WorkerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WorkerError::Scan(error) => Some(error),
            WorkerError::Execution(error) => Some(error),
            WorkerError::Forge(error) => Some(error),
            WorkerError::Reconcile(error) => Some(error),
            WorkerError::Apply(error) => Some(error),
            WorkerError::Agent(error) => Some(error),
            WorkerError::Ci(error) => Some(error),
            WorkerError::MultiRepo(error) => Some(error),
        }
    }
}

impl From<ScanError> for WorkerError {
    fn from(error: ScanError) -> Self {
        Self::Scan(error)
    }
}

impl From<ExecutionError> for WorkerError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl From<ForgeError> for WorkerError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

impl From<ReconcileError> for WorkerError {
    fn from(error: ReconcileError) -> Self {
        Self::Reconcile(error)
    }
}

impl From<ApplyError> for WorkerError {
    fn from(error: ApplyError) -> Self {
        Self::Apply(error)
    }
}

impl From<AgentError> for WorkerError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}

impl From<CiError> for WorkerError {
    fn from(error: CiError) -> Self {
        Self::Ci(error)
    }
}
