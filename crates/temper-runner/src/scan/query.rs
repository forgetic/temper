//! Read-only artifact querying and work-item assembly for scans.

use crate::observability::gate_summary;
use chrono::{DateTime, Utc};
use std::time::Instant;
use temper_forge::{Forge, Issue, PullRequest, RepositoryId};
use temper_log::emit::{CiCompleted, GateEvaluated, emit_ci_completed, emit_gate_evaluated};
use temper_log::{WorkItemRef, strip_provider_scheme};
use temper_workflow::plan::{matches_queue_cheap, matches_queue_with};
use temper_workflow::{
    ArtifactSource, CiState, CiStatus, ClassifiedArtifact, Classifier, CompiledWorkflow,
    ExecutionError, GateSignals, QueueId, QueueManifest, RoleId, SignalNeeds, ValidatedWorkflow,
    queue_active, requires_human_attention,
};

use super::candidate::{
    self, CandidateQueryPlan, ScanMode, candidate_query_plan, candidate_query_plan_for_roles,
};
use super::{AutomatedWorkItem, ScanError, TargetedArtifactSnapshot, WorkItem};

pub(super) async fn scan_inner<F: Forge + ?Sized>(
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
    let artifacts = read_artifacts(
        forge,
        repo,
        workflow,
        &queues,
        &query_plan,
        "role",
        scan_mode_name(mode),
        false,
    )
    .await?;
    Ok(work_items(&queues, &artifacts, now, role_filter))
}

pub(super) async fn scan_roles_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
    roles: &[RoleId],
    mode: ScanMode,
) -> Result<Vec<WorkItem>, ScanError> {
    let queues = candidate::queues_for_roles(compiled, roles);
    if queues.is_empty() {
        return Ok(Vec::new());
    }

    let query_plan = candidate_query_plan_for_roles(workflow, compiled, roles, mode);
    let artifacts = read_artifacts(
        forge,
        repo,
        workflow,
        &queues,
        &query_plan,
        "role",
        scan_mode_name(mode),
        false,
    )
    .await?;
    Ok(work_items_for_roles(&queues, &artifacts, now, roles))
}

pub(super) async fn scan_automated_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: DateTime<Utc>,
) -> Result<Vec<AutomatedWorkItem>, ScanError> {
    let queues = candidate::queues_for_scan(compiled, None, ScanMode::Automated);
    if queues.is_empty() {
        return Ok(Vec::new());
    }

    let query_plan = candidate_query_plan(workflow, compiled, None, ScanMode::Automated);
    let artifacts = read_artifacts(
        forge,
        repo,
        workflow,
        &queues,
        &query_plan,
        "mechanical",
        "automation",
        true,
    )
    .await?;
    Ok(automated_work_items(&queues, &artifacts, now))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn targeted_role_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    snapshot: &TargetedArtifactSnapshot,
    classified: ClassifiedArtifact,
    roles: &[RoleId],
    now: DateTime<Utc>,
) -> Result<(Vec<WorkItem>, Option<CiStatus>), ScanError> {
    let queues = candidate::queues_for_roles(compiled, roles);
    let needs_ci = queues
        .iter()
        .any(|queue| matches_queue_cheap(*queue, &classified) && SignalNeeds::for_queue(*queue).ci);
    let artifacts =
        targeted_artifacts(forge, repo, workflow, &queues, snapshot, classified, false).await?;
    let ci_status = needs_ci
        .then(|| artifacts.first().map(|artifact| *artifact.signals.ci()))
        .flatten();
    Ok((
        work_items_for_roles(&queues, &artifacts, now, roles),
        ci_status,
    ))
}

pub(super) async fn targeted_automated_inner<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    snapshot: &TargetedArtifactSnapshot,
    classified: ClassifiedArtifact,
    now: DateTime<Utc>,
) -> Result<Vec<AutomatedWorkItem>, ScanError> {
    let queues = candidate::queues_for_scan(compiled, None, ScanMode::Automated);
    let artifacts =
        targeted_artifacts(forge, repo, workflow, &queues, snapshot, classified, true).await?;
    Ok(automated_work_items(&queues, &artifacts, now))
}

async fn targeted_artifacts<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    queues: &[&QueueManifest],
    snapshot: &TargetedArtifactSnapshot,
    classified: ClassifiedArtifact,
    emit_ci_completed: bool,
) -> Result<Vec<ScannedArtifact>, ScanError> {
    if queues.is_empty() {
        return Ok(Vec::new());
    }
    if snapshot.source() != classified.source {
        return Err(ScanError::InvalidWorkflow(
            "targeted snapshot does not match its classification".to_string(),
        ));
    }
    let mut artifacts = Vec::new();
    push_candidate(
        forge,
        repo,
        workflow,
        queues,
        classified,
        Some(snapshot),
        &mut artifacts,
        emit_ci_completed,
    )
    .await?;
    Ok(artifacts)
}

#[allow(clippy::too_many_arguments)]
async fn read_artifacts<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    queues: &[&QueueManifest],
    query_plan: &CandidateQueryPlan,
    consumer: &'static str,
    scope: &'static str,
    emit_ci_completed: bool,
) -> Result<Vec<ScannedArtifact>, ScanError> {
    let classifier = Classifier::new(workflow);
    let mut artifacts = Vec::new();
    let (issues, pull_requests) =
        read_candidate_summaries(forge, repo, query_plan, consumer, scope).await?;

    for issue in issues {
        let Ok(classified) = classifier.classify_issue(&issue) else {
            continue;
        };
        push_candidate(
            forge,
            repo,
            workflow,
            queues,
            classified,
            None,
            &mut artifacts,
            emit_ci_completed,
        )
        .await?;
    }

    for pull_request in pull_requests {
        let Ok(classified) = classifier.classify_pull_request(&pull_request) else {
            continue;
        };
        push_candidate(
            forge,
            repo,
            workflow,
            queues,
            classified,
            None,
            &mut artifacts,
            emit_ci_completed,
        )
        .await?;
    }

    artifacts.sort_by_key(scanned_order_key);
    Ok(artifacts)
}

async fn read_candidate_summaries<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    query_plan: &CandidateQueryPlan,
    consumer: &'static str,
    scope: &'static str,
) -> Result<(Vec<Issue>, Vec<PullRequest>), ScanError> {
    let started = Instant::now();
    let provider_requests_before = forge.provider_request_count();
    let logical_bucket_count = query_plan
        .issue_queries
        .len()
        .saturating_add(query_plan.pull_request_queries.len());
    let mut issues = Vec::new();
    let mut pull_requests = Vec::new();
    let result: Result<(), ScanError> = async {
        for query in &query_plan.issue_queries {
            issues.extend(forge.list_issue_candidates(repo, query.clone()).await?);
        }
        for query in &query_plan.pull_request_queries {
            pull_requests.extend(
                forge
                    .list_pull_request_candidates(repo, query.clone())
                    .await?,
            );
        }
        Ok(())
    }
    .await;

    normalize_issue_candidates(&mut issues);
    normalize_pull_request_candidates(&mut pull_requests);
    let unique_count = issues.len().saturating_add(pull_requests.len());
    let provider_requests = provider_requests_before.and_then(|before| {
        forge
            .provider_request_count()
            .map(|after| after.saturating_sub(before))
    });
    let outcome = if result.is_ok() { "success" } else { "failed" };
    tracing::debug!(
        target: "temper::worker",
        measurement = "candidate.discovery",
        repo = strip_provider_scheme(repo.as_str()),
        candidate.consumer = consumer,
        candidate.scope = scope,
        candidate.logical_bucket_count = saturating_u64(logical_bucket_count),
        candidate.provider_request_total = provider_requests.unwrap_or(0),
        candidate.provider_requests_available = provider_requests.is_some(),
        candidate.unique_count = saturating_u64(unique_count),
        outcome,
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "candidate discovery {outcome} for {consumer}/{scope}"
    );
    result?;
    Ok((issues, pull_requests))
}

fn normalize_issue_candidates(issues: &mut Vec<Issue>) {
    issues.sort_by(|left, right| {
        left.number
            .cmp(&right.number)
            .then_with(|| left.id.cmp(&right.id))
    });
    issues.dedup_by(|left, right| left.id == right.id);
}

fn normalize_pull_request_candidates(pull_requests: &mut Vec<PullRequest>) {
    pull_requests.sort_by(|left, right| {
        left.number
            .cmp(&right.number)
            .then_with(|| left.id.cmp(&right.id))
    });
    pull_requests.dedup_by(|left, right| left.id == right.id);
}

fn scan_mode_name(mode: ScanMode) -> &'static str {
    match mode {
        ScanMode::Normal => "normal",
        ScanMode::Wake => "wake",
        ScanMode::Automated => "automation",
        ScanMode::Audit => "audit",
    }
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
async fn push_candidate<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    queues: &[&QueueManifest],
    classified: ClassifiedArtifact,
    snapshot: Option<&TargetedArtifactSnapshot>,
    artifacts: &mut Vec<ScannedArtifact>,
    emit_ci_completed: bool,
) -> Result<(), ScanError> {
    // Staging and human-attention state are global automation barriers. Check
    // both before any queue-specific gate read so custom workflows cannot
    // accidentally bypass them.
    if classified.metadata.staged || requires_human_attention(&classified.labels) {
        return Ok(());
    }
    let Some(needs) = signal_needs_for_candidate(queues, &classified) else {
        return Ok(());
    };
    let (classified, signals) = if needs.is_empty() {
        (classified, GateSignals::default())
    } else if let Some(snapshot) = snapshot {
        let executor = workflow.executor(forge);
        let signals = match snapshot {
            TargetedArtifactSnapshot::Issue(issue) => {
                executor
                    .read_classified_issue_gate_signals_with_needs(repo, issue, &classified, needs)
                    .await
            }
            TargetedArtifactSnapshot::PullRequest(pull_request) => {
                executor
                    .read_classified_pull_request_gate_signals_with_needs(
                        repo,
                        pull_request,
                        &classified,
                        needs,
                    )
                    .await
            }
        }?;
        (classified, signals)
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

    emit_pr_gate_evaluated(repo, &classified, &signals, needs, emit_ci_completed);
    // Broad signal reads return a freshly classified artifact. Re-check the
    // attention barrier so a park that became visible during candidate
    // enrichment cannot produce work from the stale summary.
    if requires_human_attention(&classified.labels) {
        return Ok(());
    }
    artifacts.push(ScannedArtifact {
        classified,
        signals,
    });
    Ok(())
}

/// Emits §7 engine observability for a CI-gated pull-request candidate whose
/// gates were just read freshly from the forge.
///
/// Only pull requests on a CI-gated track (`needs.ci`) produce these lines. When
/// the fresh aggregate CI signal is terminal during an automated scan, the scan
/// emits `ci.completed` for the PR before the gate summary. The `gate.evaluated`
/// line is emitted for both pending and terminal reads: while CI is pending the
/// note is `waiting on CI`; once CI passes it becomes `-> queue 'landing'
/// eligible to land`.
///
/// This fires per scan pass that re-reads a gated PR's signals, so an idle PR
/// awaiting CI re-emits on each backstop tick. `emit_gate_evaluated` deliberately
/// keeps these repeatable read-side observations at debug; actual merge
/// execution is measured separately as a mechanical landing attempt.
fn emit_pr_gate_evaluated(
    repo: &RepositoryId,
    classified: &ClassifiedArtifact,
    signals: &GateSignals,
    needs: SignalNeeds,
    emit_ci_completed_event: bool,
) {
    if !needs.ci {
        return;
    }
    let ArtifactSource::PullRequest { number } = classified.source else {
        return;
    };
    let item = WorkItemRef::pull_request(strip_provider_scheme(repo.as_str()), number.get());
    if emit_ci_completed_event {
        match signals.ci().state() {
            CiState::Passed => emit_ci_completed(CiCompleted {
                item: &item,
                conclusion: "success",
                duration_ms: 0,
                trigger_source: None,
                detection_latency_ms: None,
                queue: None,
                role: None,
            }),
            CiState::Failed => emit_ci_completed(CiCompleted {
                item: &item,
                conclusion: "failure",
                duration_ms: 0,
                trigger_source: None,
                detection_latency_ms: None,
                queue: None,
                role: None,
            }),
            CiState::Pending => {}
        }
    }

    let gates = gate_summary(signals);
    let note = if signals.ci().is_passed() {
        "-> queue 'landing' eligible to land"
    } else {
        "waiting on CI"
    };
    emit_gate_evaluated(GateEvaluated {
        item: &item,
        gates: &gates,
        note,
    });
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

        let members = active_members(queue, artifacts, now);
        for member in members {
            emit_member_items(queue, member, role_filter.map(|(role, _)| role), &mut items);
        }
    }
    items
}

fn work_items_for_roles(
    queues: &[&QueueManifest],
    artifacts: &[ScannedArtifact],
    now: DateTime<Utc>,
    roles: &[RoleId],
) -> Vec<WorkItem> {
    let mut items = Vec::new();
    for &queue in queues {
        for member in active_members(queue, artifacts, now) {
            for subscriber in &queue.subscribers {
                if roles.contains(subscriber) {
                    items.push(WorkItem {
                        queue: queue.id.clone(),
                        role: subscriber.clone(),
                        target: member.source,
                        kind: member.kind.clone(),
                    });
                }
            }
        }
    }
    items
}

fn automated_work_items(
    queues: &[&QueueManifest],
    artifacts: &[ScannedArtifact],
    now: DateTime<Utc>,
) -> Vec<AutomatedWorkItem> {
    let mut items = Vec::new();
    for &queue in queues {
        let Some(automation) = &queue.automation else {
            continue;
        };
        for member in active_members(queue, artifacts, now) {
            items.push(AutomatedWorkItem {
                queue: queue.id.clone(),
                actor: automation.actor.clone(),
                transition: automation.transition.clone(),
                executor: automation.executor.clone(),
                outcomes: automation.outcomes.clone(),
                target: member.source,
                kind: member.kind.clone(),
            });
        }
    }
    items
}

fn active_members<'a>(
    queue: &QueueManifest,
    artifacts: &'a [ScannedArtifact],
    now: DateTime<Utc>,
) -> Vec<&'a ClassifiedArtifact> {
    let members: Vec<&ClassifiedArtifact> = artifacts
        .iter()
        .filter(|artifact| matches_queue_with(queue, &artifact.classified, &artifact.signals))
        .map(|artifact| &artifact.classified)
        .collect();
    if queue_active(queue, &members, now) {
        members
    } else {
        Vec::new()
    }
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
