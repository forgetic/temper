// SPDX-License-Identifier: MPL-2.0

//! Safe, runtime-bound plan-validation audit descriptors.

use sha2::{Digest, Sha256};
use temper_forge::{Forge, RepositoryId, UserId};
use temper_log::emit::{ValidationOutcome, ValidationOutcomeKind, emit_validation_outcome};
use temper_log::validation_summary_preview;
use temper_protocol_worker::{ArtifactSummary, ArtifactType, JobContext, JobResult};
use temper_runner::artifact_ref;
use temper_workflow::{ArtifactSource, TransitionCompletionAudit, TransitionId};

use crate::InFlightJob;
use crate::applier::ApplyOutcome;
use crate::forge_applier::ForgeApplier;

const VALIDATION_ACTION: &str = "validate_plan";
const VALIDATION_MARKER_DOMAIN: &[u8] = b"temper:plan-validation-assignment:v1";
const MAX_SCOPE_REFERENCES: usize = 50;
const MAX_INLINE_CHARS: usize = 160;

/// Safe facts retained beside the workflow runtime's completion comment.
pub(super) struct ValidationAudit {
    pub(super) completion: TransitionCompletionAudit,
    pub(super) actor_id: UserId,
    outcome: ValidationOutcomeKind,
    actor_handle: String,
    job_id: String,
    transition: String,
    correlation_key: String,
    scope_count: usize,
    follow_up_count: usize,
    summary_preview: String,
}

impl ValidationAudit {
    /// Emits the typed outcome only after the executor reports that its audit
    /// and routed source transition both converged.
    pub(super) fn emit(
        &self,
        repository_id: &RepositoryId,
        source: ArtifactSource,
        workflow_role: &str,
    ) {
        let item = artifact_ref(repository_id, source);
        emit_validation_outcome(ValidationOutcome {
            item: &item,
            outcome: self.outcome,
            workflow_role,
            forge_actor_handle: &self.actor_handle,
            forge_actor_id: self.actor_id.as_str(),
            job_id: &self.job_id,
            transition: &self.transition,
            correlation_key: &self.correlation_key,
            validation_scope_count: self.scope_count,
            follow_up_count: self.follow_up_count,
            summary: &self.summary_preview,
        });
    }
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Builds the audit only from the action contract and safe result fields.
    /// Routed transition names are data, never action detectors.
    pub(super) async fn build_validation_audit(
        &self,
        job: &InFlightJob,
        context: &JobContext,
        result: &JobResult,
        routed: &TransitionId,
    ) -> Result<Option<ValidationAudit>, ApplyOutcome> {
        if context.action.as_deref() != Some(VALIDATION_ACTION) {
            return Ok(None);
        }
        let outcome = match result.verdict.as_deref() {
            Some("validated") => ValidationOutcomeKind::Validated,
            Some("needs_followup") => ValidationOutcomeKind::NeedsFollowup,
            // Preserve compatibility with custom workflows that happen to use
            // this action name but do not implement the plan-validation
            // outcome contract.
            _ => return Ok(None),
        };

        // Resolve the authenticated identity before any executor mutation. A
        // missing identity makes audit publication retryable rather than
        // allowing a transition without attributable evidence.
        let actor =
            self.forge
                .current_user()
                .await
                .map_err(|error| ApplyOutcome::ConvergencePending {
                    reason: format!("could not resolve validation audit actor: {error}"),
                })?;
        let summary_preview = validation_summary_preview(result.summary.as_deref().unwrap_or(""));
        let correlation_key = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.trim())
            .filter(|key| !key.is_empty())
            .unwrap_or("unavailable")
            .to_string();
        let scope = context
            .artifact_context
            .as_ref()
            .map(|bundle| bundle.validation_scope.as_slice())
            .unwrap_or_default();
        let marker = format!(
            "<!-- temper:comment-key=plan-validation:{} -->",
            assignment_marker_key(&job.job_id, job.attempt_id.as_deref())
        );
        let body = render_validation_audit(
            &marker,
            outcome,
            &summary_preview,
            &job.role,
            &actor.handle,
            actor.id.as_str(),
            &job.job_id,
            job.attempt_id.as_deref(),
            routed.as_str(),
            &correlation_key,
            scope,
        );

        Ok(Some(ValidationAudit {
            completion: TransitionCompletionAudit::new(marker, body),
            actor_id: actor.id,
            outcome,
            actor_handle: actor.handle,
            job_id: job.job_id.clone(),
            transition: routed.as_str().to_string(),
            correlation_key,
            scope_count: scope.len(),
            follow_up_count: result.children.len(),
            summary_preview,
        }))
    }
}

#[allow(clippy::too_many_arguments)]
fn render_validation_audit(
    marker: &str,
    outcome: ValidationOutcomeKind,
    summary_preview: &str,
    workflow_role: &str,
    actor_handle: &str,
    actor_id: &str,
    job_id: &str,
    attempt_id: Option<&str>,
    transition: &str,
    correlation_key: &str,
    scope: &[ArtifactSummary],
) -> String {
    let summary = if summary_preview.is_empty() {
        "(not provided)".to_string()
    } else {
        escape_html(summary_preview)
    };
    let attempt_id = attempt_id
        .map(bounded_inline)
        .unwrap_or_else(|| "legacy-unfenced".to_string());
    let mut body = format!(
        "## Plan validation outcome\n\n**Outcome:** `{}`  \n**Summary:** {}\n\n- Workflow role: `{}`\n- Forge actor: `{}` (`{}`)\n- Job ID: `{}`\n- Attempt ID: `{}`\n- Routed transition: `{}`\n- Workspace coordination key: `{}`",
        outcome.as_str(),
        summary,
        bounded_inline(workflow_role),
        bounded_inline(actor_handle),
        bounded_inline(actor_id),
        bounded_inline(job_id),
        attempt_id,
        bounded_inline(transition),
        bounded_inline(correlation_key),
    );

    body.push_str("\n\n### Validation scope (artifact bodies omitted)\n");
    if scope.is_empty() {
        body.push_str("- No implementation artifacts were recorded.");
    } else {
        for artifact in scope.iter().take(MAX_SCOPE_REFERENCES) {
            let kind = match artifact.artifact.artifact_type {
                ArtifactType::Issue => "issue",
                ArtifactType::PullRequest => "pull request",
            };
            body.push_str(&format!(
                "- {}#{} ({kind})\n",
                bounded_inline(&artifact.artifact.repository.path),
                artifact.artifact.number,
            ));
        }
        if scope.len() > MAX_SCOPE_REFERENCES {
            body.push_str(&format!(
                "- … {} additional bounded references omitted\n",
                scope.len() - MAX_SCOPE_REFERENCES
            ));
        }
        while body.ends_with('\n') {
            body.pop();
        }
    }
    body.push_str("\n\n");
    body.push_str(marker);
    body
}

fn bounded_inline(value: &str) -> String {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "'");
    let mut chars = normalized.chars();
    let mut bounded = chars.by_ref().take(MAX_INLINE_CHARS).collect::<String>();
    if chars.next().is_some() {
        bounded.pop();
        bounded.push('…');
    }
    bounded
}

/// Returns a compact, unambiguous key for the exact durable attempt.
///
/// Job IDs identify a queue slot and are intentionally reused when a plan
/// returns to validation. The optional attempt fence is therefore part of the
/// marker identity. Length-prefixing keeps arbitrary opaque values distinct;
/// hashing keeps the hidden comment bounded and prevents marker injection.
fn assignment_marker_key(job_id: &str, attempt_id: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(VALIDATION_MARKER_DOMAIN);
    hash_marker_component(&mut hasher, job_id.as_bytes());
    match attempt_id {
        Some(attempt_id) => {
            hasher.update([1]);
            hash_marker_component(&mut hasher, attempt_id.as_bytes());
        }
        None => hasher.update([0]),
    }
    let digest = hasher.finalize();
    format!("assignment-sha256:{digest:x}")
}

fn hash_marker_component(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_markers_are_stable_distinct_attempt_identities() {
        let first = assignment_marker_key("same-job", Some("attempt-1"));
        assert_eq!(first, assignment_marker_key("same-job", Some("attempt-1")));
        assert_ne!(first, assignment_marker_key("same-job", Some("attempt-2")));
        assert_ne!(first, assignment_marker_key("same-job", None));
        assert!(first.starts_with("assignment-sha256:"));
        assert_eq!(first.len(), "assignment-sha256:".len() + 64);
    }

    #[test]
    fn assignment_marker_encoding_has_no_component_boundary_collisions() {
        assert_ne!(
            assignment_marker_key("ab", Some("c")),
            assignment_marker_key("a", Some("bc"))
        );
    }

    #[test]
    fn inline_values_are_normalized_and_character_bounded() {
        let bounded = bounded_inline(&format!("  `{}`\n", "é".repeat(200)));
        assert_eq!(bounded.chars().count(), MAX_INLINE_CHARS);
        assert!(bounded.ends_with('…'));
        assert!(!bounded.contains('`'));
    }
}
