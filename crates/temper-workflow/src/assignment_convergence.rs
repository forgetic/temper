//! Fresh-state convergence for abandoned durable assignments.
//!
//! Startup recovery and live reconciliation share this implementation so they
//! cannot disagree about contract validation, dependency projection, PR-head
//! recovery, quarantine, or assignment fencing.

use std::error::Error;
use std::fmt;

use chrono::{DateTime, Utc};
use temper_forge::{Forge, ForgeError, RepositoryId, UpdateIssue, UpdatePullRequest};

use crate::artifact::ArtifactTarget;
use crate::classify::{ArtifactSource, ClassifiedArtifact, Classifier};
use crate::dependency_state;
use crate::ids::ArtifactKindId;
use crate::lease::{LeaseError, LeaseManager, LeasePolicy};
use crate::metadata::{DurableAssignment, WorkflowMetadata, parse_metadata_block};
use crate::relation::RelationKind;
use crate::validated::{Effect, ValidatedWorkflow};

mod audit;
mod observability;
mod pr;
use audit::{has_assignment, publish_assignment_recovery_audit};
use observability::emit_assignment_convergence;
pub use pr::recover_advanced_pull_request_assignment_from_durable;

/// Stable marker used to make assignment-recovery audit comments idempotent.
pub const ASSIGNMENT_RECOVERY_AUDIT_MARKER: &str =
    "<!-- temper:comment-key=startup_assignment_recovery -->";

/// Result of validating a durable assignment against fresh Forge state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignmentValidation {
    /// The assignment is current and its workflow contract is unambiguous.
    Valid {
        kind: ArtifactKindId,
        expires_at: DateTime<Utc>,
    },
    /// Fresh state no longer names the captured assignment snapshot.
    Stale,
    /// The captured assignment was invalid and has been parked for inspection.
    Quarantined,
}

/// Result of converging one captured durable assignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentConvergenceOutcome {
    /// Issue state or an ordinary PR claim was rolled back.
    Converged,
    /// A worker-pushed PR head was recovered through its declared transition.
    AdvancedHeadRecovered,
    /// Fresh state no longer names the captured assignment snapshot.
    Stale,
    /// An invalid or ambiguous contract was parked for human inspection.
    Quarantined,
}

/// Failure to read or mutate Forge while converging an assignment.
#[derive(Debug)]
pub enum AssignmentConvergenceError {
    Forge(ForgeError),
    Lease(LeaseError),
    InvalidContract(String),
}

impl fmt::Display for AssignmentConvergenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forge(error) => write!(formatter, "assignment recovery Forge error: {error}"),
            Self::Lease(error) => write!(formatter, "assignment recovery lease error: {error}"),
            Self::InvalidContract(reason) => {
                write!(formatter, "invalid durable assignment contract: {reason}")
            }
        }
    }
}

impl Error for AssignmentConvergenceError {}

impl From<ForgeError> for AssignmentConvergenceError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

impl From<LeaseError> for AssignmentConvergenceError {
    fn from(error: LeaseError) -> Self {
        Self::Lease(error)
    }
}

/// Shared startup/live durable-assignment validator and converger.
pub struct AssignmentConverger<'a, F: Forge + ?Sized> {
    workflow: &'a ValidatedWorkflow,
    forge: &'a F,
    leases: LeaseManager<'a, F>,
}

impl<'a, F: Forge + ?Sized> AssignmentConverger<'a, F> {
    pub fn new(workflow: &'a ValidatedWorkflow, forge: &'a F, policy: LeasePolicy) -> Self {
        Self {
            workflow,
            forge,
            leases: LeaseManager::new(forge, policy),
        }
    }

    /// Validates fresh target, assignment, action, role, kind, and queue state.
    /// Invalid contracts are quarantined through the same path used by
    /// convergence; a replacement assignment is a stale no-op.
    pub async fn validate_current(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<AssignmentValidation, AssignmentConvergenceError> {
        match self.load_validated(repo, target, expected).await? {
            LoadedValidation::Valid(validated) => Ok(AssignmentValidation::Valid {
                kind: validated.contract.kind,
                expires_at: validated.contract.expires_at,
            }),
            LoadedValidation::Stale => Ok(AssignmentValidation::Stale),
            LoadedValidation::Invalid(reason) => {
                match self
                    .quarantine_assignment(repo, target, expected, &reason)
                    .await?
                {
                    AssignmentConvergenceOutcome::Stale => Ok(AssignmentValidation::Stale),
                    _ => Ok(AssignmentValidation::Quarantined),
                }
            }
        }
    }

    /// Reloads and fully converges an abandoned assignment. All mutation paths
    /// require the complete captured assignment snapshot, including expiry, so
    /// a heartbeat or newer assignment cannot be cleared by an old report.
    pub async fn converge(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<AssignmentConvergenceOutcome, AssignmentConvergenceError> {
        let result = self.converge_inner(repo, target, expected).await;
        emit_assignment_convergence(repo, target, expected, &result);
        result
    }

    async fn converge_inner(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<AssignmentConvergenceOutcome, AssignmentConvergenceError> {
        let validated = match self.load_validated(repo, target, expected).await? {
            LoadedValidation::Valid(validated) => validated,
            LoadedValidation::Stale => return Ok(AssignmentConvergenceOutcome::Stale),
            LoadedValidation::Invalid(reason) => {
                return self
                    .quarantine_assignment(repo, target, expected, &reason)
                    .await;
            }
        };

        match validated.artifact {
            ValidatedArtifact::Issue(artifact) => {
                let dependency_status =
                    dependency_state::status_for_artifact(self.forge, repo, &artifact).await;
                let dependencies_unresolved = artifact
                    .relations
                    .iter()
                    .filter(|relation| relation.kind == RelationKind::Dependency)
                    .any(|relation| !dependency_status.is_landed(&relation.target));
                match self
                    .leases
                    .converge_issue_assignment_snapshot(
                        repo,
                        target,
                        expected,
                        &validated.contract.queue_labels,
                        &validated.contract.claim_labels,
                        dependencies_unresolved,
                    )
                    .await
                {
                    Ok(()) => Ok(AssignmentConvergenceOutcome::Converged),
                    Err(LeaseError::AssignmentConflict { .. }) => {
                        Ok(AssignmentConvergenceOutcome::Stale)
                    }
                    Err(error) => Err(error.into()),
                }
            }
            ValidatedArtifact::PullRequest(_) => {
                if recover_advanced_pull_request_assignment_from_durable(
                    self.forge,
                    repo,
                    target,
                    expected,
                    validated.contract.kind,
                    self.workflow,
                )
                .await?
                {
                    return Ok(AssignmentConvergenceOutcome::AdvancedHeadRecovered);
                }
                match self
                    .leases
                    .rollback_assignment_snapshot(repo, target, expected)
                    .await
                {
                    Ok(()) => Ok(AssignmentConvergenceOutcome::Converged),
                    Err(LeaseError::AssignmentConflict { .. }) => {
                        Ok(AssignmentConvergenceOutcome::Stale)
                    }
                    Err(error) => Err(error.into()),
                }
            }
        }
    }

    /// Quarantines a captured assignment after a higher-level startup
    /// reconstruction check finds an impossible contract.
    pub async fn quarantine_current(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        reason: &str,
    ) -> Result<AssignmentConvergenceOutcome, AssignmentConvergenceError> {
        self.quarantine_assignment(repo, target, expected, reason)
            .await
    }

    /// Adds the idempotent attention label/comment used when no assignment can
    /// be safely interpreted. This is also used by startup inventory when the
    /// metadata block itself is malformed. A freshly parseable assignment or a
    /// concurrent artifact update makes an older target-level finding a no-op.
    pub async fn quarantine_target(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        reason: &str,
    ) -> Result<(), AssignmentConvergenceError> {
        match target {
            ArtifactSource::Issue { number } => {
                let issue = self
                    .forge
                    .get_issue_by_number(repo, number)
                    .await?
                    .ok_or_else(|| ForgeError::NotFound(format!("issue {number}")))?;
                if has_assignment(&issue.body) {
                    return Ok(());
                }
                if !issue.labels.iter().any(|label| label == "needs-human") {
                    match self
                        .forge
                        .update_issue(
                            &issue.id,
                            UpdateIssue {
                                add_labels: vec!["needs-human".to_string()],
                                expected_version: Some(issue.version),
                                ..UpdateIssue::default()
                            },
                        )
                        .await
                    {
                        Ok(_) => {}
                        Err(ForgeError::Conflict(_)) => return Ok(()),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            ArtifactSource::PullRequest { number } => {
                let pull_request = self
                    .forge
                    .get_pull_request_by_number(repo, number)
                    .await?
                    .ok_or_else(|| ForgeError::NotFound(format!("pull request {number}")))?;
                if has_assignment(&pull_request.body) {
                    return Ok(());
                }
                if !pull_request
                    .labels
                    .iter()
                    .any(|label| label == "needs-human")
                {
                    match self
                        .forge
                        .update_pull_request(
                            &pull_request.id,
                            UpdatePullRequest {
                                add_labels: vec!["needs-human".to_string()],
                                expected_version: Some(pull_request.version),
                                ..UpdatePullRequest::default()
                            },
                        )
                        .await
                    {
                        Ok(_) => {}
                        Err(ForgeError::Conflict(_)) => return Ok(()),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        publish_assignment_recovery_audit(self.workflow, self.forge, repo, target, reason, true)
            .await
    }

    async fn quarantine_assignment(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        reason: &str,
    ) -> Result<AssignmentConvergenceOutcome, AssignmentConvergenceError> {
        match self
            .leases
            .quarantine_assignment_snapshot(repo, target, expected)
            .await
        {
            Ok(()) => {}
            Err(LeaseError::AssignmentConflict { .. }) => {
                return Ok(AssignmentConvergenceOutcome::Stale);
            }
            Err(error) => return Err(error.into()),
        }
        publish_assignment_recovery_audit(self.workflow, self.forge, repo, target, reason, true)
            .await?;
        Ok(AssignmentConvergenceOutcome::Quarantined)
    }

    async fn load_validated(
        &self,
        repo: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<LoadedValidation, AssignmentConvergenceError> {
        let (metadata, artifact) = match target {
            ArtifactSource::Issue { number } => {
                let Some(issue) = self.forge.get_issue_by_number(repo, number).await? else {
                    return Ok(LoadedValidation::Stale);
                };
                let metadata = match parse_metadata_block(&issue.body) {
                    Ok(Some(metadata)) => metadata,
                    Ok(None) => return Ok(LoadedValidation::Stale),
                    Err(error) => {
                        return Ok(LoadedValidation::Invalid(format!(
                            "malformed workflow metadata: {error}"
                        )));
                    }
                };
                let classified = match Classifier::new(self.workflow).classify_issue(&issue) {
                    Ok(artifact) => artifact,
                    Err(error) => {
                        return Ok(LoadedValidation::Invalid(format!(
                            "assigned issue is ambiguous: {error}"
                        )));
                    }
                };
                (metadata, ValidatedArtifact::Issue(classified))
            }
            ArtifactSource::PullRequest { number } => {
                let Some(pull_request) =
                    self.forge.get_pull_request_by_number(repo, number).await?
                else {
                    return Ok(LoadedValidation::Stale);
                };
                let metadata = match parse_metadata_block(&pull_request.body) {
                    Ok(Some(metadata)) => metadata,
                    Ok(None) => return Ok(LoadedValidation::Stale),
                    Err(error) => {
                        return Ok(LoadedValidation::Invalid(format!(
                            "malformed workflow metadata: {error}"
                        )));
                    }
                };
                let classified =
                    match Classifier::new(self.workflow).classify_pull_request(&pull_request) {
                        Ok(artifact) => artifact,
                        Err(error) => {
                            return Ok(LoadedValidation::Invalid(format!(
                                "assigned pull request is ambiguous: {error}"
                            )));
                        }
                    };
                (metadata, ValidatedArtifact::PullRequest(classified))
            }
        };

        let Some(current) = metadata.assignment.as_ref() else {
            return Ok(LoadedValidation::Stale);
        };
        if current != expected {
            return Ok(LoadedValidation::Stale);
        }
        let resolved_kind = match &artifact {
            ValidatedArtifact::Issue(classified) | ValidatedArtifact::PullRequest(classified) => {
                &classified.kind
            }
        };
        let contract = match self.contract(target, resolved_kind, &metadata, expected) {
            Ok(contract) => contract,
            Err(reason) => return Ok(LoadedValidation::Invalid(reason)),
        };
        Ok(LoadedValidation::Valid(ValidatedAssignment {
            artifact,
            contract,
        }))
    }

    fn contract(
        &self,
        target: ArtifactSource,
        resolved_kind: &ArtifactKindId,
        metadata: &WorkflowMetadata,
        assignment: &DurableAssignment,
    ) -> Result<AssignmentContract, String> {
        require_nonempty(&assignment.job_id, "job id")?;
        require_nonempty(&assignment.worker_id, "worker id")?;
        require_nonempty(&assignment.daemon_boot_id, "daemon boot id")?;
        let kind = resolved_kind.clone();
        let declared_kind = self
            .workflow
            .artifact_kind(&kind)
            .ok_or_else(|| format!("durable assignment names unknown artifact kind `{kind}`"))?;
        let target_matches = matches!(
            (target, declared_kind.target),
            (ArtifactSource::Issue { .. }, ArtifactTarget::Issue)
                | (
                    ArtifactSource::PullRequest { .. },
                    ArtifactTarget::PullRequest
                )
        );
        if !target_matches {
            return Err(format!(
                "durable assignment kind `{kind}` does not match its Forge artifact"
            ));
        }

        let role = assignment
            .role
            .as_ref()
            .ok_or_else(|| "durable assignment is missing role".to_string())?;
        let action = assignment
            .action
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "durable assignment is missing action".to_string())?;
        let transition = self
            .workflow
            .transitions()
            .iter()
            .find(|transition| transition.id.as_str() == action)
            .ok_or_else(|| format!("durable assignment names unknown action `{action}`"))?;
        if transition.artifact != kind || !transition.roles.contains(role) {
            return Err(format!(
                "durable assignment role `{role}` is not authorized for action `{action}` on `{kind}`"
            ));
        }

        let queue_name = assignment
            .queue
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "durable assignment is missing queue".to_string())?;
        let queue = self
            .workflow
            .queues()
            .iter()
            .find(|queue| queue.id.as_str() == queue_name)
            .ok_or_else(|| format!("durable assignment names unknown queue `{queue_name}`"))?;
        if !queue.artifacts.contains(&kind) {
            return Err(format!(
                "durable assignment queue `{queue_name}` does not contain `{kind}`"
            ));
        }
        let role_config = self
            .workflow
            .roles()
            .iter()
            .find(|candidate| &candidate.id == role)
            .ok_or_else(|| format!("durable assignment names unknown role `{role}`"))?;
        if !role_config.queues.contains(&queue.id) {
            return Err(format!(
                "durable assignment role `{role}` does not subscribe to queue `{queue_name}`"
            ));
        }
        if !queue.actions.is_empty() {
            let matching = queue
                .actions
                .iter()
                .filter(|candidate| &candidate.role == role)
                .filter(|candidate| {
                    candidate
                        .artifact
                        .as_ref()
                        .is_none_or(|artifact| artifact == &kind)
                })
                .collect::<Vec<_>>();
            if matching.len() != 1 || matching[0].action.as_str() != action {
                return Err(format!(
                    "durable assignment action `{action}` does not unambiguously match queue `{queue_name}` for role `{role}`"
                ));
            }
        }

        let expires_at = assignment
            .expires_at
            .or_else(|| metadata.lease.as_ref().map(|lease| lease.expires_at))
            .ok_or_else(|| "durable assignment is missing expiry".to_string())?;
        let mut queue_labels = queue
            .labels
            .iter()
            .map(|label| label.as_str().to_string())
            .collect::<Vec<_>>();
        if let Some(branch) = queue.any_of.iter().find(|branch| {
            branch.labels.iter().all(|label| {
                assignment
                    .pre_claim_labels
                    .iter()
                    .any(|present| present == label.as_str())
            })
        }) {
            for label in &branch.labels {
                push_unique_string(&mut queue_labels, label.as_str());
            }
        }
        let claim_labels = transition
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::AddLabel(label) => Some(label.as_str().to_string()),
                _ => None,
            })
            .collect();
        Ok(AssignmentContract {
            kind,
            expires_at,
            queue_labels,
            claim_labels,
        })
    }
}

fn require_nonempty(value: &Option<String>, name: &str) -> Result<(), String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|_| ())
        .ok_or_else(|| format!("durable assignment is missing {name}"))
}

struct ValidatedAssignment {
    artifact: ValidatedArtifact,
    contract: AssignmentContract,
}

struct AssignmentContract {
    kind: ArtifactKindId,
    expires_at: DateTime<Utc>,
    queue_labels: Vec<String>,
    claim_labels: Vec<String>,
}

enum ValidatedArtifact {
    Issue(ClassifiedArtifact),
    PullRequest(ClassifiedArtifact),
}

enum LoadedValidation {
    Valid(ValidatedAssignment),
    Stale,
    Invalid(String),
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}
