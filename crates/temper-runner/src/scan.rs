//! Forge-backed queue scanning.
//!
//! A scan is read-only: it lists Forge artifacts, classifies what cleanly
//! belongs to the workflow, lazily reads runtime gate signals for cheap-matched
//! queue candidates, and emits work for active queues. Classification failures
//! are left to the workflow reconciler and do not fail the scan.

mod candidate;
mod query;

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use temper_forge_model::{Forge, ForgeError, RepositoryId};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, ClassifiedArtifact, CompiledWorkflow, ExecutionError,
    ExternalToolId, QueueId, RoleId, TransitionId, ValidatedWorkflow, VerdictId,
};

pub use candidate::{CandidateQueryPlan, ScanMode, candidate_query_plan};

/// A role-addressed member of an active queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkItem {
    /// Queue that selected the artifact.
    pub queue: QueueId,
    /// Role subscribed to the queue.
    pub role: RoleId,
    /// Forge artifact to service.
    pub target: ArtifactSource,
    /// Workflow artifact kind resolved during classification.
    pub kind: ArtifactKindId,
}

/// A mechanically serviced member of an active automated queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomatedWorkItem {
    /// Queue whose automation metadata selected the artifact.
    pub queue: QueueId,
    /// Workflow role whose authority should execute the transition.
    pub actor: RoleId,
    /// Transition declared by the queue automation metadata.
    pub transition: TransitionId,
    /// Optional workspace executor (declared external-tool id) that services
    /// this automation directly. When set, the mechanical worker invokes the
    /// workspace bound for `actor` under this id and routes on its verdict
    /// through `outcomes`; when `None` the worker runs `transition` directly.
    pub executor: Option<ExternalToolId>,
    /// Verdict id -> transition id routing declared by the automation. The
    /// merge-conflict fallback lives here under the built-in merge-conflict
    /// verdict (see [`VerdictId::merge_conflict`]).
    pub outcomes: BTreeMap<VerdictId, TransitionId>,
    /// Forge artifact to service.
    pub target: ArtifactSource,
    /// Workflow artifact kind resolved during classification.
    pub kind: ArtifactKindId,
}

/// Errors that can stop a scan.
#[derive(Debug)]
pub enum ScanError {
    /// Listing Forge state failed.
    Forge(ForgeError),
    /// Reading runtime gate signals failed.
    Execution(ExecutionError),
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Forge(error) => write!(formatter, "forge scan failed: {error}"),
            ScanError::Execution(error) => write!(formatter, "signal read failed: {error}"),
        }
    }
}

impl Error for ScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ScanError::Forge(error) => Some(error),
            ScanError::Execution(error) => Some(error),
        }
    }
}

impl From<ForgeError> for ScanError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

impl From<ExecutionError> for ScanError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

/// Scans all workflow-visible artifacts and returns work for every role.
///
/// The result is deterministic: queue declaration order, then artifact number,
/// then the queue's subscriber order. The scan does not mutate Forge state.
pub async fn scan<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(forge, repo, workflow, compiled, now, None, ScanMode::Normal).await
}

/// Scans all workflow-visible artifacts and returns work for one role.
///
/// Unknown roles have no subscribed queues and therefore receive no work.
pub async fn scan_role<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: &RoleId,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        Some(role),
        ScanMode::Normal,
    )
    .await
}

/// Wake-triggered scan for one role. Queue scoping stays role-bounded while
/// adding narrow terminal/recovery interest for workflow labels.
pub async fn scan_role_wake<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: &RoleId,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        Some(role),
        ScanMode::Wake,
    )
    .await
}

/// Scans active queues that declare mechanical automation metadata.
///
/// The scan is read-only and bounded by the automated queues' candidate query
/// plan. Results are deterministic by queue declaration order and then artifact
/// number. The automation actor is not required to subscribe to the queue.
pub async fn scan_automated_queues<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
) -> Result<Vec<AutomatedWorkItem>, ScanError> {
    query::scan_automated_inner(forge, repo, workflow, compiled, now).await
}

/// Returns automated queue items for one already-classified artifact, reusing
/// the same signal read, queue matching, activity and automation metadata logic
/// as the broad automated scan.
pub async fn targeted_automated_work_items<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    classified: ClassifiedArtifact,
    now: DateTime<Utc>,
) -> Result<Vec<AutomatedWorkItem>, ScanError> {
    query::targeted_automated_inner(forge, repo, workflow, compiled, classified, now).await
}

/// Runs a broad audit scan for all workflow queues and workflow-labelled
/// recovery interest while still avoiding unlabelled closed history.
pub async fn scan_audit<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(forge, repo, workflow, compiled, now, None, ScanMode::Audit).await
}

/// Runs an audit scan but emits work only for one role.
pub async fn scan_role_audit<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: &RoleId,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        Some(role),
        ScanMode::Audit,
    )
    .await
}
