//! Forge-backed queue scanning.
//!
//! A scan is read-only: it lists Forge artifacts, classifies what cleanly
//! belongs to the workflow, lazily reads runtime gate signals for cheap-matched
//! queue candidates, and emits work for active queues. Classification failures
//! are left to the workflow reconciler and do not fail the scan.

mod candidate;

use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use temper_forge::{Forge, ForgeError, Issue, PullRequest, RepositoryId};
use temper_workflow::plan::{matches_queue_cheap, matches_queue_with};
use temper_workflow::{
    queue_active, ArtifactKindId, ArtifactSource, ClassifiedArtifact, Classifier, CompiledWorkflow,
    ExecutionError, GateSignals, QueueId, QueueManifest, RoleId, SignalNeeds, ValidatedWorkflow,
};

pub use candidate::{candidate_query_plan, CandidateQueryPlan, ScanMode};

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
    scan_inner(forge, repo, workflow, compiled, now, None, ScanMode::Normal).await
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
    scan_inner(
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

/// Runs a broad audit scan for all workflow queues and workflow-labelled
/// recovery interest while still avoiding unlabelled closed history.
pub async fn scan_audit<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
) -> Result<Vec<WorkItem>, ScanError> {
    scan_inner(forge, repo, workflow, compiled, now, None, ScanMode::Audit).await
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
    scan_inner(
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

async fn scan_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: Option<&RoleId>,
    mode: ScanMode,
) -> Result<Vec<WorkItem>, ScanError> {
    let role_filter = match role {
        Some(id) => match compiled.role(id) {
            Some(manifest) => Some((id, manifest.queues.as_slice())),
            None => return Ok(Vec::new()),
        },
        None => None,
    };

    let queues = candidate::queues_for_scan(compiled, role, mode);
    if queues.is_empty() {
        return Ok(Vec::new());
    }

    let query_plan = candidate_query_plan(workflow, compiled, role, mode);
    let artifacts = read_artifacts(forge, repo, workflow, &queues, &query_plan).await?;
    Ok(work_items(&queues, &artifacts, now, role_filter))
}

async fn read_artifacts<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    queues: &[&QueueManifest],
    query_plan: &CandidateQueryPlan,
) -> Result<Vec<ScannedArtifact>, ScanError> {
    let classifier = Classifier::new(workflow);
    let mut artifacts = Vec::new();
    let mut seen_issues = HashSet::new();
    let mut seen_pull_requests = HashSet::new();

    for query in &query_plan.issue_queries {
        for issue in forge.list_issues(repo, query.clone()).await? {
            if !seen_issues.insert(issue_key(&issue)) {
                continue;
            }
            let Ok(classified) = classifier.classify_issue(&issue) else {
                continue;
            };
            push_candidate(forge, repo, workflow, queues, classified, &mut artifacts).await?;
        }
    }

    for query in &query_plan.pull_request_queries {
        for pull_request in forge.list_pull_requests(repo, query.clone()).await? {
            if !seen_pull_requests.insert(pull_request_key(&pull_request)) {
                continue;
            }
            let Ok(classified) = classifier.classify_pull_request(&pull_request) else {
                continue;
            };
            push_candidate(forge, repo, workflow, queues, classified, &mut artifacts).await?;
        }
    }

    artifacts.sort_by_key(scanned_order_key);
    Ok(artifacts)
}

fn issue_key(issue: &Issue) -> (temper_forge::IssueId, temper_forge::ItemNumber) {
    (issue.id.clone(), issue.number)
}

fn pull_request_key(
    pull_request: &PullRequest,
) -> (temper_forge::PullRequestId, temper_forge::ItemNumber) {
    (pull_request.id.clone(), pull_request.number)
}

async fn push_candidate<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    queues: &[&QueueManifest],
    classified: ClassifiedArtifact,
    artifacts: &mut Vec<ScannedArtifact>,
) -> Result<(), ScanError> {
    let Some(needs) = signal_needs_for_candidate(queues, &classified) else {
        return Ok(());
    };
    let (classified, signals) = if needs.is_empty() {
        (classified, GateSignals::default())
    } else {
        match workflow
            .executor(forge)
            .read_classified_gate_signals_with_needs(repo, classified.source, needs)
            .await
        {
            Ok(fresh) => fresh,
            Err(ExecutionError::Classification(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    };
    artifacts.push(ScannedArtifact {
        classified,
        signals,
    });
    Ok(())
}

fn signal_needs_for_candidate(
    queues: &[&QueueManifest],
    artifact: &ClassifiedArtifact,
) -> Option<SignalNeeds> {
    let mut matched = false;
    let mut needs = SignalNeeds::none();
    for &queue in queues {
        if matches_queue_cheap(queue, artifact) {
            matched = true;
            needs = needs.union(SignalNeeds::for_queue(queue));
        }
    }
    matched.then_some(needs)
}

fn work_items(
    queues: &[&QueueManifest],
    artifacts: &[ScannedArtifact],
    now: DateTime<Utc>,
    role_filter: Option<(&RoleId, &[QueueId])>,
) -> Vec<WorkItem> {
    let mut items = Vec::new();
    for &queue in queues {
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
