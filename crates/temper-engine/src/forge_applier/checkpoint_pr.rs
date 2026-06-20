// SPDX-License-Identifier: MPL-2.0

//! Early implementation-PR materialization from diff-bearing checkpoint
//! progress.
//!
//! Final success remains the canonical handoff and can open every repo outcome
//! the worker reports. Checkpoint progress is deliberately narrower: the wire
//! marker only proves that *some* writable repo pushed a checkpoint SHA. When we
//! can safely map that proof to one repo (single-repo jobs, or a workspace with
//! exactly one writable repo), we ensure the same implementation PR that final
//! success would later reuse. For multi-writable workspaces the checkpoint marker
//! does not identify which repo changed, so final success completes the set.

use std::collections::BTreeMap;

use temper_forge::{Forge, Issue, ItemNumber, Repository};
use temper_protocol_worker::{Branch, JobContext, JobProgress, RepoOutcome};
use temper_runner::pr_correlation_key;
use temper_workflow::{ArtifactKindId, ArtifactSource};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::coordinated::{
    CoordinatedSet, coordinated_landing_order, manifest_depends_on,
};
use crate::forge_applier::run_ledger::RunLedgerPullRequest;
use crate::workflow_meta::{implementation_pr_create_labels, implementation_pr_labels};

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Ensures an implementation PR from a checkpoint that safely proves a
    /// pushed product diff for a known writable repo. Returns the PRs ensured so
    /// the source-issue run ledger can hand off immediately.
    pub(super) async fn ensure_checkpoint_implementation_prs(
        &self,
        job: &InFlightJob,
        primary_repository: &Repository,
        issue: &Issue,
        progress: &JobProgress,
    ) -> Vec<RunLedgerPullRequest> {
        let Some(pushed_sha) = pr_worthy_checkpoint_sha(progress) else {
            return Vec::new();
        };
        if !self.final_progress_uses_implementation_pr_body(job) {
            return Vec::new();
        }

        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %issue.number,
                    %error,
                    "forge applier could not parse JobContext for checkpoint PR"
                );
                return Vec::new();
            }
        };
        if !primary_checkout_is_writable(&context) {
            return Vec::new();
        }

        let source_kind = ArtifactKindId::new(context.artifact_kind.clone());
        let coordination_key = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.clone())
            .unwrap_or_else(|| pr_correlation_key(&source_kind, issue.number));
        if progress.correlation_key != coordination_key {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %issue.number,
                progress_correlation_key = %progress.correlation_key,
                coordination_key = %coordination_key,
                "forge applier skipped checkpoint PR with mismatched correlation key"
            );
            return Vec::new();
        }

        let outcomes = checkpoint_repo_outcomes(job, &context, &coordination_key, pushed_sha);
        if outcomes.is_empty() {
            return Vec::new();
        }

        let lookup_labels = implementation_pr_labels(self.workflow.as_ref());
        let create_labels = implementation_pr_create_labels(self.workflow.as_ref());
        let depends_on = manifest_depends_on(&context);
        let order = coordinated_landing_order(&outcomes, &depends_on);
        let summary = checkpoint_pr_summary(progress);
        let mut opened = BTreeMap::new();
        let set = CoordinatedSet {
            job,
            primary_id: &primary_repository.id,
            issue_title: &issue.title,
            number: issue.number,
            summary: &summary,
            coordination_key: &coordination_key,
            lookup_labels: &lookup_labels,
            create_labels: &create_labels,
            depends_on: &depends_on,
        };
        for index in order {
            self.open_coordinated_pr(&set, &outcomes[index], &mut opened)
                .await;
        }

        opened
            .into_iter()
            .map(|(repo, (_, number))| RunLedgerPullRequest { repo, number })
            .collect()
    }
}

fn pr_worthy_checkpoint_sha(progress: &JobProgress) -> Option<&str> {
    if progress.state != "done" || looks_like_scaffold_checkpoint(&progress.status) {
        return None;
    }
    progress
        .pushed_sha
        .as_deref()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
}

fn looks_like_scaffold_checkpoint(status: &str) -> bool {
    let normalized = status
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "publish_plan"
            | "publish plan"
            | "publish implementation plan"
            | "implementation plan"
            | "plan implementation"
    ) || normalized.contains("plan-first")
        || normalized.contains("empty pr")
}

fn primary_checkout_is_writable(context: &JobContext) -> bool {
    context
        .checkout_capability
        .as_deref()
        .map(str::trim)
        .filter(|capability| !capability.is_empty())
        .is_none_or(|capability| capability == "writable")
}

fn checkpoint_repo_outcomes(
    job: &InFlightJob,
    context: &JobContext,
    coordination_key: &str,
    pushed_sha: &str,
) -> Vec<RepoOutcome> {
    if let Some(workspace) = &context.workspace {
        let writable = workspace.writable().collect::<Vec<_>>();
        let [repo] = writable.as_slice() else {
            if writable.len() > 1 {
                tracing::debug!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    writable_repos = writable.len(),
                    "forge applier deferred multi-repo checkpoint PRs until final success because progress does not identify changed repos"
                );
            }
            return Vec::new();
        };
        return vec![RepoOutcome {
            repo: repo.repo.clone(),
            branch: Branch {
                name: repo
                    .branch_hint
                    .clone()
                    .unwrap_or_else(|| default_work_branch(coordination_key)),
                head_sha: pushed_sha.to_string(),
            },
        }];
    }

    vec![RepoOutcome {
        repo: job.repo.clone(),
        branch: Branch {
            name: default_work_branch(coordination_key),
            head_sha: pushed_sha.to_string(),
        },
    }]
}

fn default_work_branch(coordination_key: &str) -> String {
    format!("agent/{coordination_key}")
}

fn checkpoint_pr_summary(progress: &JobProgress) -> String {
    let status = progress.status.split_whitespace().collect::<Vec<_>>().join(" ");
    if status.is_empty() {
        "Opened from a pushed checkpoint. The final implementation summary will update this PR when the run completes.".to_string()
    } else {
        format!(
            "Opened from pushed checkpoint step {} (`{}`). The final implementation summary will update this PR when the run completes.",
            progress.step, status
        )
    }
}
