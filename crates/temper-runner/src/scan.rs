//! Forge-backed queue scanning.
//!
//! A scan is read-only: it lists Forge artifacts, classifies what cleanly
//! belongs to the workflow, lazily reads runtime gate signals for cheap-matched
//! queue candidates, and emits work for active queues. Classification failures
//! are left to the workflow reconciler and do not fail the scan.

mod candidate;
mod ci;
mod discovery;
mod discovery_state;
mod query;

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use temper_forge::{
    Forge, ForgeError, HintArtifactKind, Issue, IssueState, ItemNumber, PullRequest,
    PullRequestState, RepositoryId,
};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, CiStatus, ClassifiedArtifact, CompiledWorkflow, ExecutionError,
    ExternalToolId, QueueId, RoleId, TransitionId, ValidatedWorkflow, VerdictId,
};

pub use candidate::{
    CandidateQueryPlan, ScanMode, candidate_query_plan, candidate_query_plan_for_roles,
};
pub use ci::{CiStatusObservation, read_ci_status_observations};
pub use discovery_state::{
    TerminalDiscoveryBucket, TerminalDiscoveryBucketSnapshot, TerminalDiscoveryCommitOutcome,
    TerminalDiscoveryContinuation, TerminalDiscoveryPageCommit, TerminalDiscoveryPolicy,
    TerminalDiscoverySnapshot, TerminalDiscoveryState, TerminalDiscoveryStateError,
};
pub use query::TerminalDiscoveryRead;

/// Explicit issue-or-pull-request address used by item-scoped scans.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactAddress {
    pub kind: HintArtifactKind,
    pub number: ItemNumber,
}

impl ArtifactAddress {
    pub const fn new(kind: HintArtifactKind, number: ItemNumber) -> Self {
        Self { kind, number }
    }

    pub const fn issue(number: ItemNumber) -> Self {
        Self::new(HintArtifactKind::Issue, number)
    }

    pub const fn pull_request(number: ItemNumber) -> Self {
        Self::new(HintArtifactKind::PullRequest, number)
    }

    pub const fn source(self) -> ArtifactSource {
        match self.kind {
            HintArtifactKind::Issue => ArtifactSource::Issue {
                number: self.number,
            },
            HintArtifactKind::PullRequest => ArtifactSource::PullRequest {
                number: self.number,
            },
        }
    }
}

/// The exact Forge representation loaded for an item-scoped scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetedArtifactSnapshot {
    Issue(Box<Issue>),
    PullRequest(Box<PullRequest>),
}

impl TargetedArtifactSnapshot {
    pub fn source(&self) -> ArtifactSource {
        match self {
            Self::Issue(issue) => ArtifactSource::Issue {
                number: issue.number,
            },
            Self::PullRequest(pull_request) => ArtifactSource::PullRequest {
                number: pull_request.number,
            },
        }
    }

    pub fn is_open(&self) -> bool {
        match self {
            Self::Issue(issue) => issue.state == IssueState::Open,
            Self::PullRequest(pull_request) => pull_request.state == PullRequestState::Open,
        }
    }
}

/// One exact load and its single classification result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedTargetedArtifact {
    pub snapshot: TargetedArtifactSnapshot,
    pub classified: ClassifiedArtifact,
}

/// Result of evaluating one artifact for a configured set of roles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetedRoleScan {
    pub snapshot: TargetedArtifactSnapshot,
    pub classified: ClassifiedArtifact,
    /// Fresh aggregate CI status when at least one evaluated role queue needs
    /// CI. `None` means the exact scan did not read CI.
    pub ci_status: Option<CiStatus>,
    pub work_items: Vec<WorkItem>,
}

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
    /// Workflow declarations did not identify a safe role-worker action.
    InvalidWorkflow(String),
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Forge(error) => write!(formatter, "forge scan failed: {error}"),
            ScanError::Execution(error) => write!(formatter, "signal read failed: {error}"),
            ScanError::InvalidWorkflow(error) => {
                write!(formatter, "invalid workflow scan: {error}")
            }
        }
    }
}

impl Error for ScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ScanError::Forge(error) => Some(error),
            ScanError::Execution(error) => Some(error),
            ScanError::InvalidWorkflow(_) => None,
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
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        None,
        ScanMode::Normal,
        None,
    )
    .await
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
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn scan_role_with_discovery<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: &RoleId,
    discovery: &TerminalDiscoveryState,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        Some(role),
        ScanMode::Normal,
        Some((discovery, TerminalDiscoveryRead::Advance)),
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
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn scan_role_wake_with_discovery<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: &RoleId,
    discovery: &TerminalDiscoveryState,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        Some(role),
        ScanMode::Wake,
        Some((discovery, TerminalDiscoveryRead::Advance)),
    )
    .await
}

/// Runs one broad recovery-inclusive scan for the union of queues subscribed
/// by `roles`, sharing candidate and signal reads across subscribers.
pub async fn scan_roles_wake<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    roles: &[RoleId],
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_roles_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        roles,
        ScanMode::Wake,
        None,
    )
    .await
}

/// Shared-state broad role scan. Mechanical and role lanes use
/// [`TerminalDiscoveryRead::RetainedOnly`] after another lane has already
/// advanced the generation's bounded provider page.
#[allow(clippy::too_many_arguments)]
pub async fn scan_roles_wake_with_discovery<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    roles: &[RoleId],
    discovery: &TerminalDiscoveryState,
    terminal_read: TerminalDiscoveryRead,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_roles_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        roles,
        ScanMode::Wake,
        Some((discovery, terminal_read)),
    )
    .await
}

/// Starts a new poll sweep only after the previous one became authoritative.
/// Incomplete continuations are never reset by a reconstructed consumer.
pub fn prepare_terminal_discovery_generation(
    discovery: &TerminalDiscoveryState,
    repo: &RepositoryId,
) -> bool {
    discovery
        .snapshot(repo)
        .is_some_and(|snapshot| snapshot.authoritative)
        && discovery.invalidate_repository(repo)
}

/// Registers one exact webhook/local-mutation target without making the hint
/// authoritative. The next broad generation still advances provider polling.
pub fn retain_terminal_discovery_target(
    discovery: &TerminalDiscoveryState,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    target: ArtifactAddress,
) -> Result<bool, ScanError> {
    let plan = candidate_query_plan(workflow, compiled, None, ScanMode::Wake);
    discovery::initialize_terminal_discovery(discovery, repo, workflow, &plan)?;
    discovery
        .retain_exact_target(repo, target)
        .map_err(|error| ScanError::InvalidWorkflow(error.to_string()))
}

pub(crate) async fn read_reconciliation_candidates<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    now: DateTime<Utc>,
    discovery: &TerminalDiscoveryState,
    exact_targets: &[ArtifactAddress],
) -> Result<(Vec<Issue>, Vec<PullRequest>), ScanError> {
    let reconciliation = temper_workflow::reconciliation_candidate_query_plan(workflow);
    let plan = CandidateQueryPlan {
        issue_queries: reconciliation.issue_queries,
        pull_request_queries: reconciliation.pull_request_queries,
    };
    discovery::read_candidate_summaries(
        forge,
        repo,
        workflow,
        &plan,
        "mechanical",
        "reconciliation",
        now,
        Some((discovery, TerminalDiscoveryRead::Advance)),
        exact_targets,
        true,
    )
    .await
}

/// Loads and classifies exactly one explicitly typed artifact.
///
/// Missing or unclassifiable artifacts are ordinary targeted misses. The
/// selected namespace is never probed through the other exact-fetch API.
pub async fn load_targeted_artifact<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    address: ArtifactAddress,
) -> Result<Option<LoadedTargetedArtifact>, ScanError> {
    let classifier = temper_workflow::Classifier::new(workflow);
    let loaded = match address.kind {
        HintArtifactKind::Issue => {
            let Some(issue) = forge.get_issue_by_number(repo, address.number).await? else {
                return Ok(None);
            };
            let Ok(classified) = classifier.classify_issue(&issue) else {
                return Ok(None);
            };
            LoadedTargetedArtifact {
                snapshot: TargetedArtifactSnapshot::Issue(Box::new(issue)),
                classified,
            }
        }
        HintArtifactKind::PullRequest => {
            let Some(pull_request) = forge
                .get_pull_request_by_number(repo, address.number)
                .await?
            else {
                return Ok(None);
            };
            let Ok(classified) = classifier.classify_pull_request(&pull_request) else {
                return Ok(None);
            };
            LoadedTargetedArtifact {
                snapshot: TargetedArtifactSnapshot::PullRequest(Box::new(pull_request)),
                classified,
            }
        }
    };
    Ok(Some(loaded))
}

/// Loads one artifact and evaluates only queues subscribed by `roles`.
///
/// Queue and subscriber order remains workflow-declaration order. All cheap
/// matches are assembled before one unioned signal read, and staged artifacts
/// are rejected before any signal operation.
#[allow(clippy::too_many_arguments)]
pub async fn targeted_role_work_items<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    address: ArtifactAddress,
    roles: &[RoleId],
    now: DateTime<Utc>,
) -> Result<Option<TargetedRoleScan>, ScanError> {
    let Some(loaded) = load_targeted_artifact(forge, repo, workflow, address).await? else {
        return Ok(None);
    };
    let (work_items, ci_status) = query::targeted_role_inner(
        forge,
        repo,
        workflow,
        compiled,
        &loaded.snapshot,
        loaded.classified.clone(),
        roles,
        now,
    )
    .await?;
    Ok(Some(TargetedRoleScan {
        snapshot: loaded.snapshot,
        classified: loaded.classified,
        ci_status,
        work_items,
    }))
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

/// Returns automated queue items for one already-loaded and classified
/// artifact, reusing the same signal read, queue matching, activity and
/// automation metadata logic as targeted role evaluation.
pub async fn targeted_automated_work_items<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    loaded: &LoadedTargetedArtifact,
    now: DateTime<Utc>,
) -> Result<Vec<AutomatedWorkItem>, ScanError> {
    query::targeted_automated_inner(
        forge,
        repo,
        workflow,
        compiled,
        &loaded.snapshot,
        loaded.classified.clone(),
        now,
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
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        None,
        ScanMode::Audit,
        None,
    )
    .await
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
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn scan_role_audit_with_discovery<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    role: &RoleId,
    discovery: &TerminalDiscoveryState,
) -> Result<Vec<WorkItem>, ScanError> {
    query::scan_inner(
        forge,
        repo,
        workflow,
        compiled,
        now,
        Some(role),
        ScanMode::Audit,
        Some((discovery, TerminalDiscoveryRead::Advance)),
    )
    .await
}
