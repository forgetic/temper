// SPDX-License-Identifier: MPL-2.0

//! Verdict-path application: route a worker verdict through the compiled
//! workflow to a declared transition and execute it against the source issue or
//! pull request, binding any `create_issues` children the verdict produced and
//! metadata-driven `create_pull_request` inputs declared by the routed issue
//! transition.

use temper_forge::{Forge, ItemNumber, RepositoryId, RepositoryPath};
use temper_protocol_worker::{JobChild, JobResult};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, Classifier, Effect, ExecutionContext, ExecutionError, Executor,
    RoleId, TransitionId, ValidatedWorkflow, VerdictId,
};

use temper_log::emit::{
    QueueEntered, TransitionApplied, emit_queue_entered, emit_transition_applied,
};
use temper_runner::{artifact_ref, labels_delta, queue_after_transition, workspace_content_key};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::verdict_pr::VerdictPullRequestBinding;

/// Carries the arguments [`ForgeApplier::execute_routed_verdict`] needs without a
/// long positional signature.
pub(super) struct RoutedVerdictApply<'a> {
    pub(super) job: &'a InFlightJob,
    pub(super) repository_id: &'a RepositoryId,
    pub(super) source: ArtifactSource,
    pub(super) routed: &'a TransitionId,
    pub(super) role_id: &'a RoleId,
    pub(super) action: &'a str,
    pub(super) verdict: &'a str,
    pub(super) artifact_label: &'static str,
    pub(super) number: ItemNumber,
    pub(super) context: ExecutionContext,
}

/// Carries the arguments [`ForgeApplier::bind_create_issues_children`] needs.
pub(super) struct VerdictChildrenBinding<'a> {
    pub(super) job: &'a InFlightJob,
    pub(super) repository_id: &'a RepositoryId,
    pub(super) artifact_kind: &'a str,
    pub(super) source_body: &'a str,
    pub(super) routed: &'a TransitionId,
    pub(super) number: ItemNumber,
    pub(super) children: Vec<JobChild>,
    pub(super) context: &'a mut ExecutionContext,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_verdict(&self, job: InFlightJob, result: JobResult) {
        let Some(verdict) = result.verdict.clone() else {
            return;
        };

        let job_context = match serde_json::from_value::<temper_protocol_worker::JobContext>(
            job.job_payload.clone(),
        ) {
            Ok(context) => context,
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    %error,
                    "forge applier could not parse JobContext"
                );
                return;
            }
        };
        let Some(action) = job_context.action.as_deref() else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                role = %job.role,
                verdict = %verdict,
                "forge applier could not route verdict: missing action in JobContext"
            );
            return;
        };

        let role_id = RoleId::new(job.role.as_str());
        let verdict_id = VerdictId::new(verdict.as_str());
        let Some(role) = self.compiled.role(&role_id) else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                role = %job.role,
                action = %action,
                verdict = %verdict,
                "forge applier could not route verdict: role not found in compiled workflow"
            );
            return;
        };
        let Some(tool) = role.tools.iter().find(|tool| tool.name == action) else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                role = %job.role,
                action = %action,
                verdict = %verdict,
                "forge applier could not route verdict: action not found in compiled workflow"
            );
            return;
        };
        let Some(routed) = tool.outcomes.get(&verdict_id).cloned() else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                role = %job.role,
                action = %action,
                verdict = %verdict,
                "forge applier could not route verdict: verdict is not declared for action"
            );
            return;
        };

        match job.artifact.kind.as_str() {
            "issue" => {
                self.apply_issue_verdict(
                    &job,
                    &job_context,
                    &role_id,
                    action,
                    &verdict,
                    &routed,
                    result,
                )
                .await;
            }
            "pull_request" => {
                self.apply_pull_request_verdict(
                    &job,
                    &job_context,
                    &role_id,
                    action,
                    &verdict,
                    &routed,
                    result,
                )
                .await;
            }
            _ => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    artifact_kind = %job.artifact.kind,
                    artifact_item = %job.artifact.item,
                    "forge applier ignored unsupported verdict job"
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_issue_verdict(
        &self,
        job: &InFlightJob,
        job_context: &temper_protocol_worker::JobContext,
        role_id: &RoleId,
        action: &str,
        verdict: &str,
        routed: &TransitionId,
        result: JobResult,
    ) {
        let Some((repository, issue)) = self.resolve_issue(job).await else {
            return;
        };
        let number = issue.number;
        let source_kind = ArtifactKindId::new(job_context.artifact_kind.as_str());
        // A routed outcome such as intake -> code changes identifying labels. On
        // replay the current kind no longer matches the queued source kind, so
        // treat it as stale before the executor would classify the request as a
        // validation error.
        if matches!(
            Classifier::new(self.workflow.as_ref()).classify_issue(&issue),
            Ok(classified) if classified.kind != source_kind
        ) {
            return;
        }

        let result_title = result.title.clone();
        let result_body = result.body.clone();
        let result_children = result.children;
        let mut context = verdict_execution_context(
            self.forge.as_ref(),
            job,
            &job_context.artifact_kind,
            routed,
            role_id,
            number,
            result_body.clone(),
        )
        .await;
        if !result_children.is_empty()
            && !self
                .bind_create_issues_children(VerdictChildrenBinding {
                    job,
                    repository_id: &repository.id,
                    artifact_kind: &job_context.artifact_kind,
                    source_body: &issue.body,
                    routed,
                    number,
                    children: result_children,
                    context: &mut context,
                })
                .await
        {
            return;
        }
        if !self.bind_metadata_pull_request_creates(VerdictPullRequestBinding {
            job,
            repository: &repository,
            issue: &issue,
            artifact_kind: &job_context.artifact_kind,
            routed,
            number,
            title: result_title.as_deref(),
            body: result_body.as_deref(),
            context: &mut context,
        }) {
            return;
        }

        self.execute_routed_verdict(RoutedVerdictApply {
            job,
            repository_id: &repository.id,
            source: ArtifactSource::Issue { number },
            routed,
            role_id,
            action,
            verdict,
            artifact_label: "issue",
            number,
            context,
        })
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_pull_request_verdict(
        &self,
        job: &InFlightJob,
        job_context: &temper_protocol_worker::JobContext,
        role_id: &RoleId,
        action: &str,
        verdict: &str,
        routed: &TransitionId,
        result: JobResult,
    ) {
        let Some((repository, pull_request)) = self.resolve_pull_request(job).await else {
            return;
        };
        let number = pull_request.number;
        let source_kind = ArtifactKindId::new(job_context.artifact_kind.as_str());
        // Replay after the routed transition changes the PR's identifying kind is
        // stale in the same way as the issue path above. Classifications that fail
        // for ordinary stale/terminal state are left for the executor's stale
        // mapping.
        if matches!(
            Classifier::new(self.workflow.as_ref()).classify_pull_request(&pull_request),
            Ok(classified) if classified.kind != source_kind
        ) {
            return;
        }

        let context = verdict_execution_context(
            self.forge.as_ref(),
            job,
            &job_context.artifact_kind,
            routed,
            role_id,
            number,
            result.body,
        )
        .await;

        self.execute_routed_verdict(RoutedVerdictApply {
            job,
            repository_id: &repository.id,
            source: ArtifactSource::PullRequest { number },
            routed,
            role_id,
            action,
            verdict,
            artifact_label: "pull_request",
            number,
            context,
        })
        .await;
    }

    pub(super) async fn execute_routed_verdict(&self, apply: RoutedVerdictApply<'_>) {
        // Capture the coordinates the observability emit needs before the
        // execution context is moved into the executor below.
        let repository_id = apply.repository_id;
        let source = apply.source;
        match Executor::with_context(self.workflow.as_ref(), self.forge.as_ref(), apply.context)
            .execute(repository_id, source, apply.routed, apply.role_id)
            .await
        {
            Ok(report) => {
                self.emit_routed_verdict_observability(repository_id, source, &report);
            }
            Err(error) if is_stale(&error) => {}
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %apply.job.job_id,
                    repo = %apply.job.repo,
                    artifact_label = %apply.artifact_label,
                    number = %apply.number,
                    role = %apply.job.role,
                    action = %apply.action,
                    verdict = %apply.verdict,
                    routed = %apply.routed,
                    %error,
                    "forge applier could not apply routed verdict transition"
                );
            }
        }
    }

    /// Emits the §7 `engine` routed-verdict observability after the Forge
    /// transition has applied on the daemon path.
    ///
    /// Successful agent verdict outcomes such as `triage_intake_to_code` are
    /// real workflow transitions, so they emit the same `transition.applied`
    /// fact as mechanical automation. The label delta comes from the executor's
    /// applied effects. When the label effects also move the artifact into a new
    /// workflow queue, a follow-up `queue.entered` line is derived from the
    /// compiled workflow (see [`queue_after_transition`]).
    fn emit_routed_verdict_observability(
        &self,
        repository_id: &RepositoryId,
        source: ArtifactSource,
        report: &temper_workflow::ExecutionReport,
    ) {
        let item = artifact_ref(repository_id, source);
        let delta = labels_delta(&report.applied);
        emit_transition_applied(TransitionApplied {
            item: &item,
            transition: report.transition.as_str(),
            detail: "",
            labels_delta: &delta,
        });

        let Some((queue, role)) = queue_after_transition(&self.compiled, &report.applied) else {
            return;
        };
        let note = role
            .map(|role| format!("awaiting {role}"))
            .unwrap_or_default();
        emit_queue_entered(QueueEntered {
            item: &item,
            queue_to: queue.id.as_str(),
            note: &note,
        });
    }
}

pub(super) fn create_issues_effect_index(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Option<usize> {
    let declares_create_issues = workflow
        .transitions()
        .iter()
        .find(|candidate| &candidate.id == transition)?
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::CreateIssues { .. }));
    declares_create_issues.then_some(0)
}

pub(super) fn parse_child_target_repo(target_repo: &str) -> Option<RepositoryPath> {
    let (owner, name) = target_repo.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(RepositoryPath::new(owner, name))
}

async fn verdict_execution_context<F: Forge + ?Sized>(
    forge: &F,
    job: &InFlightJob,
    artifact_kind: &str,
    routed: &TransitionId,
    role_id: &RoleId,
    number: ItemNumber,
    body: Option<String>,
) -> ExecutionContext {
    let mut context = ExecutionContext::new();
    match forge.current_user().await {
        Ok(user) => {
            context.set_assignee(role_id.clone(), user.id);
        }
        Err(error) => tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            role = %job.role,
            %error,
            "forge applier could not bind current role assignee"
        ),
    }
    if let Some(body) = body {
        let content_key =
            workspace_content_key(&ArtifactKindId::new(artifact_kind), routed, number);
        context.set_set_body_at(routed.clone(), 0, body.clone());
        context.set_set_body_correlation_key_at(routed.clone(), 0, content_key.clone());
        context.set_attach_review_at(routed.clone(), 0, body);
        context.set_attach_review_correlation_key_at(routed.clone(), 0, content_key);
    }
    context
}

fn is_stale(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::TargetStale { .. }
            | ExecutionError::Classification(_)
    )
}
