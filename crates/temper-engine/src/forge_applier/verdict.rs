// SPDX-License-Identifier: MPL-2.0

//! Verdict-path application: route a worker verdict through the compiled
//! workflow to a declared transition and execute it against the source issue or
//! pull request, binding any `create_issues` children the verdict produced.

use temper_forge::{Forge, ItemNumber, RepositoryId, RepositoryPath};
use temper_worker_protocol::{JobChild, JobResult};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, Classifier, Effect, ExecutionContext, ExecutionError, Executor,
    RoleId, TransitionId, ValidatedWorkflow, VerdictId,
};

use temper_runner::workspace_content_key;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

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
    pub(super) routed: &'a TransitionId,
    pub(super) number: ItemNumber,
    pub(super) children: Vec<JobChild>,
    pub(super) context: &'a mut ExecutionContext,
}

impl<F: Forge> ForgeApplier<F> {
    pub(super) async fn apply_verdict(&self, job: InFlightJob, result: JobResult) {
        let Some(verdict) = result.verdict.clone() else {
            return;
        };

        let job_context = match serde_json::from_value::<temper_worker_protocol::JobContext>(
            job.job_payload.clone(),
        ) {
            Ok(context) => context,
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not parse JobContext for job_id={} repo={} artifact.kind={} artifact.item={}: {error}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
                return;
            }
        };
        let Some(action) = job_context.action.as_deref() else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} verdict={}: missing action in JobContext",
                job.job_id, job.repo, job.artifact.kind, job.artifact.item, job.role, verdict
            );
            return;
        };

        let role_id = RoleId::new(job.role.as_str());
        let verdict_id = VerdictId::new(verdict.as_str());
        let Some(role) = self.compiled.role(&role_id) else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} action={} verdict={}: role not found in compiled workflow",
                job.job_id,
                job.repo,
                job.artifact.kind,
                job.artifact.item,
                job.role,
                action,
                verdict
            );
            return;
        };
        let Some(tool) = role.tools.iter().find(|tool| tool.name == action) else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} action={} verdict={}: action not found in compiled workflow",
                job.job_id,
                job.repo,
                job.artifact.kind,
                job.artifact.item,
                job.role,
                action,
                verdict
            );
            return;
        };
        let Some(routed) = tool.outcomes.get(&verdict_id).cloned() else {
            eprintln!(
                "temper-daemon: forge applier could not route verdict for job_id={} repo={} artifact.kind={} artifact.item={} role={} action={} verdict={}: verdict is not declared for action",
                job.job_id,
                job.repo,
                job.artifact.kind,
                job.artifact.item,
                job.role,
                action,
                verdict
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
                eprintln!(
                    "temper-daemon: forge applier ignored unsupported verdict job for job_id={} repo={} artifact.kind={} artifact.item={}",
                    job.job_id, job.repo, job.artifact.kind, job.artifact.item
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_issue_verdict(
        &self,
        job: &InFlightJob,
        job_context: &temper_worker_protocol::JobContext,
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

        let mut context =
            verdict_execution_context(&job_context.artifact_kind, routed, number, result.body);
        if !result.children.is_empty()
            && !self
                .bind_create_issues_children(VerdictChildrenBinding {
                    job,
                    repository_id: &repository.id,
                    artifact_kind: &job_context.artifact_kind,
                    routed,
                    number,
                    children: result.children,
                    context: &mut context,
                })
                .await
        {
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
        job_context: &temper_worker_protocol::JobContext,
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

        let context =
            verdict_execution_context(&job_context.artifact_kind, routed, number, result.body);

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
        match Executor::with_context(self.workflow.as_ref(), self.forge.as_ref(), apply.context)
            .execute(
                apply.repository_id,
                apply.source,
                apply.routed,
                apply.role_id,
            )
            .await
        {
            Ok(_) => {}
            Err(error) if is_stale(&error) => {}
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not apply routed verdict transition for job_id={} repo={} {}={} role={} action={} verdict={} routed={}: {error}",
                    apply.job.job_id,
                    apply.job.repo,
                    apply.artifact_label,
                    apply.number,
                    apply.job.role,
                    apply.action,
                    apply.verdict,
                    apply.routed
                );
            }
        }
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

fn verdict_execution_context(
    artifact_kind: &str,
    routed: &TransitionId,
    number: ItemNumber,
    body: Option<String>,
) -> ExecutionContext {
    let mut context = ExecutionContext::new();
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
