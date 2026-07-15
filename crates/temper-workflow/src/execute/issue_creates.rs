//! Durable, idempotent multi-artifact issue fan-out for the [`Executor`].

mod activation;
mod create;
mod intent;
mod round;
mod wiring;

use super::verify::AppliedState;
use super::{ChildIssueCheckpoint, ExecutionError, Executor};
use crate::classify::ArtifactSource;
use crate::context::CreateIssuesChild;
use crate::ids::TransitionId;
use crate::metadata::{
    CreateIssueIntentChild, CreateIssuesCompletion, CreateIssuesIntent,
    global_child_correlation_key,
};
use std::collections::{BTreeSet, HashSet};
use std::time::{Duration, Instant};
use temper_forge::{Forge, Issue, ItemNumber, RepositoryId, UserId};

/// A concrete multi-artifact request prepared from a `CreateIssues` effect.
pub(super) struct PreparedCreateIssues {
    pub(super) transition: TransitionId,
    pub(super) effect_index: usize,
    pub(super) base_correlation_key: String,
    pub(super) children: Vec<CreateIssuesChild>,
    pub(super) record_parent_dependencies: bool,
}

/// Result of inserting or loading a durable create intent.
struct PersistedCreateIntent {
    key: String,
    newly_inserted: bool,
    intent: CreateIssuesIntent,
    parent: Issue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntentExecutionMode {
    KnownFirst,
    Recovery,
}

struct PendingIntent {
    key: String,
    intent: CreateIssuesIntent,
}

struct ResumedIntent {
    key: String,
    intent: CreateIssuesIntent,
    parent: Issue,
}

#[derive(Clone, Copy)]
enum ParentDependencyStyle {
    LegacyRepoQualified,
    Natural,
}

#[derive(Default)]
struct FanOutMetrics {
    forge_reads: u64,
    forge_writes: u64,
}

impl FanOutMetrics {
    fn read(&mut self) {
        self.forge_reads = self.forge_reads.saturating_add(1);
    }

    fn write(&mut self) {
        self.forge_writes = self.forge_writes.saturating_add(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_fan_out_completion(
    repo_id: &RepositoryId,
    parent_number: ItemNumber,
    child_count: usize,
    dependency_edge_count: usize,
    metrics: &FanOutMetrics,
    provider_requests: Option<u64>,
    recovery: bool,
    duration: Duration,
) {
    tracing::debug!(
        target: "temper::workflow",
        measurement = "fan_out.completed",
        parent.ref = %format!("{}#{}", repo_id.as_str(), parent_number.get()),
        child_count,
        dependency_edge_count,
        forge.read_total = metrics.forge_reads,
        forge.write_total = metrics.forge_writes,
        provider.request_total = provider_requests.unwrap_or(0),
        provider.requests_available = provider_requests.is_some(),
        recovery,
        duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        "workflow: issue fan-out completed"
    );
}

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Persists every fan-out before creating its first child, then executes
    /// explicit create, wiring/aggregation, activation, and completion passes.
    ///
    /// A newly inserted intent takes the known-first path and performs no
    /// correlation-history query. Existing incomplete intents take recovery,
    /// where unresolved keys are searched in one open/closed pair per target
    /// repository before any missing children are created.
    pub(super) async fn apply_issue_creates(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        parent_snapshot: Option<Issue>,
        creates: &[PreparedCreateIssues],
        completion: &CreateIssuesCompletion,
    ) -> Result<Option<AppliedState>, ExecutionError> {
        if creates.is_empty() {
            return Ok(None);
        }
        let started = Instant::now();
        let provider_requests_before = self.forge.provider_request_count();
        let child_count = creates.iter().map(|create| create.children.len()).sum();
        let dependency_edge_count = creates
            .iter()
            .flat_map(|create| &create.children)
            .map(|child| child.dependencies.len())
            .sum();
        let mut fan_out_metrics = FanOutMetrics::default();
        let mut recovery = false;
        let ArtifactSource::Issue {
            number: parent_number,
        } = target
        else {
            return Err(ExecutionError::Backend {
                message: "create_issues durability requires an issue source artifact".into(),
            });
        };
        let mut parent = parent_snapshot.ok_or_else(|| ExecutionError::Backend {
            message: "create_issues durability requires a validated source issue snapshot".into(),
        })?;
        if parent.repo_id != *repo_id || parent.number != parent_number {
            return Err(ExecutionError::Backend {
                message: "create_issues source snapshot does not match its target".into(),
            });
        }

        // Persist all sibling effects before any child mutation. This keeps a
        // multi-effect transition recoverable from the source artifact alone.
        let mut pending = Vec::new();
        for create in creates {
            let key = create_intent_key(
                target,
                &create.transition,
                create.effect_index,
                &create.base_correlation_key,
            );
            let proposed =
                self.intent_from_create(repo_id, parent_number, create, completion.clone());
            let persisted = self
                .persist_create_intent(parent, &key, proposed, &mut fan_out_metrics)
                .await?;
            parent = persisted.parent;
            if !persisted.intent.completed {
                recovery |= !persisted.newly_inserted;
                pending.push((
                    persisted.key,
                    persisted.intent,
                    if persisted.newly_inserted {
                        IntentExecutionMode::KnownFirst
                    } else {
                        IntentExecutionMode::Recovery
                    },
                ));
            }
        }

        let mut completed = Vec::new();
        for (key, intent, mode) in pending {
            let resumed = self
                .resume_create_intent(
                    repo_id,
                    parent_number,
                    &key,
                    intent,
                    mode,
                    parent,
                    &mut fan_out_metrics,
                )
                .await?;
            parent = resumed.parent;
            completed.push(PendingIntent {
                key: resumed.key,
                intent: resumed.intent,
            });
        }

        if !completed.is_empty() {
            let (committed, changed) = self
                .complete_create_intents(parent, &completed, &mut fan_out_metrics)
                .await?;
            parent = committed;
            if changed {
                self.child_issue_checkpoint(ChildIssueCheckpoint::Completed)
                    .await;
            }
        }

        let provider_requests = provider_requests_before.and_then(|before| {
            self.forge
                .provider_request_count()
                .map(|after| after.saturating_sub(before))
        });
        emit_fan_out_completion(
            repo_id,
            parent_number,
            child_count,
            dependency_edge_count,
            &fan_out_metrics,
            provider_requests,
            recovery,
            started.elapsed(),
        );

        // `persist_create_intent` returns a completed intent only when this
        // invocation's request and source completion match that durable round.
        // Otherwise it inserts a fresh round, which is completed above. A
        // returned state therefore always proves this execution's atomic source
        // commit rather than merely observing an older completed fan-out.
        Ok(Some(AppliedState {
            labels: parent.labels,
            assignees: parent.assignees,
        }))
    }

    fn intent_from_create(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        create: &PreparedCreateIssues,
        completion: CreateIssuesCompletion,
    ) -> CreateIssuesIntent {
        let children = create
            .children
            .iter()
            .map(|child| {
                let repository_id = child.target_repo.clone().unwrap_or_else(|| repo_id.clone());
                let correlation_key = if &repository_id == repo_id {
                    child_correlation_key(&create.base_correlation_key, &child.slug)
                } else {
                    global_child_correlation_key(repo_id, parent_number, &child.slug)
                };
                CreateIssueIntentChild {
                    slug: child.slug.clone(),
                    title: child.title.clone(),
                    body_hex: hex_encode(child.body.as_bytes()),
                    final_labels: normalized_labels(&child.labels),
                    dependencies: child.dependencies.clone(),
                    repository_id,
                    correlation_key,
                    number: None,
                    wired: false,
                    activated: false,
                }
            })
            .collect();
        CreateIssuesIntent {
            transition: create.transition.as_str().to_string(),
            effect_index: create.effect_index,
            correlation_key: create.base_correlation_key.clone(),
            record_parent_dependencies: create.record_parent_dependencies,
            children,
            completion: Some(completion),
            parent_wired: false,
            completed: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn resume_create_intent(
        &self,
        repo_id: &RepositoryId,
        parent_number: ItemNumber,
        key: &str,
        intent: CreateIssuesIntent,
        mode: IntentExecutionMode,
        parent: Issue,
        metrics: &mut FanOutMetrics,
    ) -> Result<ResumedIntent, ExecutionError> {
        if intent.completed {
            return Ok(ResumedIntent {
                key: key.to_string(),
                intent,
                parent,
            });
        }

        let (intent, parent, children) = self
            .create_pass(repo_id, parent_number, key, intent, mode, parent, metrics)
            .await?;
        let (intent, parent, children) = self
            .wiring_and_aggregation_pass(repo_id, key, intent, parent, children, metrics)
            .await?;
        let (intent, children) = self.activation_pass(intent, children, metrics).await?;
        debug_assert_eq!(children.len(), intent.children.len());
        Ok(ResumedIntent {
            key: key.to_string(),
            intent,
            parent,
        })
    }
}

pub(super) fn create_issues_completion(
    body: Option<&str>,
    add_labels: &[String],
    remove_labels: &[String],
    add_assignees: &[UserId],
    remove_assignees: &[UserId],
) -> CreateIssuesCompletion {
    CreateIssuesCompletion {
        body_hex: body.map(|body| hex_encode(body.as_bytes())),
        add_labels: add_labels.to_vec(),
        remove_labels: remove_labels.to_vec(),
        add_assignees: add_assignees.to_vec(),
        remove_assignees: remove_assignees.to_vec(),
    }
}

fn create_intent_key(
    target: ArtifactSource,
    transition: &TransitionId,
    effect_index: usize,
    correlation_key: &str,
) -> String {
    let source = match target {
        ArtifactSource::Issue { number } => format!("issue:{}", number.get()),
        ArtifactSource::PullRequest { number } => format!("pull_request:{}", number.get()),
    };
    format!(
        "source:{}:{}/transition:{}:{}/effect:{effect_index}/correlation:{}:{}",
        source.len(),
        source,
        transition.as_str().len(),
        transition.as_str(),
        correlation_key.len(),
        correlation_key
    )
}

fn child_correlation_key(base_correlation_key: &str, slug: &str) -> String {
    format!(
        "{}:{}/child:{}:{}",
        base_correlation_key.len(),
        base_correlation_key,
        slug.len(),
        slug
    )
}

fn normalized_labels(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| {
            let label = label.trim();
            (!label.is_empty()).then(|| label.to_string())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_intent_body(encoded: &str) -> Result<String, ExecutionError> {
    if encoded.len() % 2 != 0 {
        return Err(ExecutionError::Backend {
            message: "intent body has invalid hex encoding".into(),
        });
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_digit(pair[0]).ok_or_else(|| ExecutionError::Backend {
            message: "intent body has invalid hex encoding".into(),
        })?;
        let low = hex_digit(pair[1]).ok_or_else(|| ExecutionError::Backend {
            message: "intent body has invalid hex encoding".into(),
        })?;
        decoded.push((high << 4) | low);
    }
    String::from_utf8(decoded).map_err(|error| ExecutionError::Backend {
        message: format!("intent body is not UTF-8: {error}"),
    })
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn annotate_target_repo_error(target_repo: &RepositoryId, error: ExecutionError) -> ExecutionError {
    match error {
        ExecutionError::Backend { message } => ExecutionError::Backend {
            message: format!("cannot ensure issue in target repository `{target_repo}`: {message}"),
        },
        other => other,
    }
}

fn metadata_error(error: impl std::fmt::Display) -> ExecutionError {
    ExecutionError::Backend {
        message: format!("invalid workflow metadata: {error}"),
    }
}

/// Validates that every declared sibling dependency resolves in the same effect.
pub(super) fn validate_child_dependencies(
    transition: &TransitionId,
    effect_index: usize,
    children: &[CreateIssuesChild],
) -> Result<(), ExecutionError> {
    let slugs: HashSet<&str> = children.iter().map(|child| child.slug.as_str()).collect();
    for child in children {
        for dependency in &child.dependencies {
            if !slugs.contains(dependency.as_str()) {
                return Err(ExecutionError::UnknownCreateIssuesDependency {
                    transition: transition.clone(),
                    effect_index,
                    slug: child.slug.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }
    Ok(())
}
