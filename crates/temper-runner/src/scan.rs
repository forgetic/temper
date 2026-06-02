//! Forge-backed queue scanning.
//!
//! A scan is read-only: it lists Forge artifacts, classifies what cleanly
//! belongs to the workflow, reads runtime gate signals, and emits work for
//! active queues. Classification failures are left to the workflow reconciler
//! and do not fail the scan.

use chrono::{DateTime, Utc};
use std::error::Error;
use std::fmt;
use temper_forge::{Forge, ForgeError, IssueQuery, PullRequestQuery, RepositoryId};
use temper_workflow::plan::matches_queue_with;
use temper_workflow::{
    queue_active, ArtifactKindId, ArtifactSource, ClassifiedArtifact, Classifier, CompiledWorkflow,
    ExecutionError, GateSignals, QueueId, QueueManifest, RoleId, ValidatedWorkflow,
};

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
    scan_inner(forge, repo, workflow, compiled, now, None).await
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
    scan_inner(forge, repo, workflow, compiled, now, Some(role)).await
}

async fn scan_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: Option<&RoleId>,
) -> Result<Vec<WorkItem>, ScanError> {
    let role_filter = match role {
        Some(id) => match compiled.role(id) {
            Some(manifest) => Some((id, manifest.queues.as_slice())),
            None => return Ok(Vec::new()),
        },
        None => None,
    };

    let artifacts = read_artifacts(forge, repo, workflow).await?;
    Ok(work_items(compiled.queues(), &artifacts, now, role_filter))
}

async fn read_artifacts<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
) -> Result<Vec<ScannedArtifact>, ScanError> {
    let classifier = Classifier::new(workflow);
    let executor = workflow.executor(forge);
    let mut artifacts = Vec::new();

    for issue in forge.list_issues(repo, IssueQuery::default()).await? {
        let Ok(classified) = classifier.classify_issue(&issue) else {
            continue;
        };
        match executor.read_gate_signals(repo, classified.source).await {
            Ok(signals) => artifacts.push(ScannedArtifact {
                classified,
                signals,
            }),
            Err(ExecutionError::Classification(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }

    for pull_request in forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await?
    {
        let Ok(classified) = classifier.classify_pull_request(&pull_request) else {
            continue;
        };
        match executor.read_gate_signals(repo, classified.source).await {
            Ok(signals) => artifacts.push(ScannedArtifact {
                classified,
                signals,
            }),
            Err(ExecutionError::Classification(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }

    artifacts.sort_by_key(scanned_order_key);
    Ok(artifacts)
}

fn work_items(
    queues: &[QueueManifest],
    artifacts: &[ScannedArtifact],
    now: DateTime<Utc>,
    role_filter: Option<(&RoleId, &[QueueId])>,
) -> Vec<WorkItem> {
    let mut items = Vec::new();
    for queue in queues {
        if role_filter.is_some_and(|(_, queues)| !queues.contains(&queue.id)) {
            continue;
        }

        let members: Vec<&ClassifiedArtifact> = artifacts
            .iter()
            .filter(|artifact| matches_queue_with(queue, &artifact.classified, &artifact.signals))
            .map(|artifact| &artifact.classified)
            .collect();
        if !queue_active(queue, &members, now) {
            continue;
        }

        for member in members {
            emit_member_items(queue, member, role_filter.map(|(role, _)| role), &mut items);
        }
    }
    items
}

fn emit_member_items(
    queue: &QueueManifest,
    member: &ClassifiedArtifact,
    role_filter: Option<&RoleId>,
    items: &mut Vec<WorkItem>,
) {
    for subscriber in &queue.subscribers {
        if role_filter.is_some_and(|role| role != subscriber) {
            continue;
        }
        items.push(WorkItem {
            queue: queue.id.clone(),
            role: subscriber.clone(),
            target: member.source,
            kind: member.kind.clone(),
        });
    }
}

#[derive(Clone, Debug)]
struct ScannedArtifact {
    classified: ClassifiedArtifact,
    signals: GateSignals,
}

fn scanned_order_key(artifact: &ScannedArtifact) -> (u64, u8) {
    match artifact.classified.source {
        ArtifactSource::Issue { number } => (number.get(), 0),
        ArtifactSource::PullRequest { number } => (number.get(), 1),
    }
}
