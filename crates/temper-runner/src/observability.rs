//! Provider-neutral observability glue between runner types and `temper-log`.
//!
//! This module owns the runner-specific *inputs* to the structured event model
//! that lives in [`temper_log`]: it converts runner coordinate types
//! ([`WorkItemIdentity`], [`temper_workflow`] effects) into the plain values the
//! `temper_log::emit::*` constructors take, then the call sites emit. The
//! JSON-string-in-message renderers and the `StructuredEvent` builder that used
//! to live here are gone — fields are now real `tracing` fields produced by
//! [`temper_log`] (see the logging & observability design, §8).

mod events;
mod identity;

pub use events::{
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error, workflow_effect_summary,
};
pub use identity::{ObservabilityArtifactType, WorkItemIdentity};

use temper_forge::RepositoryId;
use temper_log::{WorkItemRef, strip_provider_scheme};
use temper_workflow::{ArtifactSource, WorkflowEffect};

/// Builds the repo-qualified [`WorkItemRef`] join key for an identity.
///
/// This is the bridge from the runner's stable [`WorkItemIdentity`] (which
/// carries the provider-qualified [`RepositoryId`](temper_forge::RepositoryId),
/// the artifact number, and its kind) to the bare `owner/repo#n` /
/// `owner/repo PR#n` form `temper-log` renders into both the human tag and the
/// `artifact.ref` machine field. The provider scheme is stripped here so call
/// sites pass an identity and get the design's canonical key.
pub fn work_item_ref(identity: &WorkItemIdentity) -> WorkItemRef {
    let repo = strip_provider_scheme(identity.repo.as_str());
    let number = identity.artifact_number.get();
    match identity.artifact_type {
        ObservabilityArtifactType::Issue => WorkItemRef::issue(repo, number),
        ObservabilityArtifactType::PullRequest => WorkItemRef::pull_request(repo, number),
    }
}

/// Builds the [`WorkItemRef`] join key from a repository id and artifact source.
///
/// The mechanical automation path services [`ArtifactSource`] targets without a
/// full [`WorkItemIdentity`]; this is the same scheme-stripping conversion as
/// [`work_item_ref`] but from the raw coordinates.
pub fn artifact_ref(repo: &RepositoryId, target: ArtifactSource) -> WorkItemRef {
    let repo = strip_provider_scheme(repo.as_str());
    match target {
        ArtifactSource::Issue { number } => WorkItemRef::issue(repo, number.get()),
        ArtifactSource::PullRequest { number } => WorkItemRef::pull_request(repo, number.get()),
    }
}

/// Renders the §7 label delta (`-untriaged +code +ready`) from applied effects.
///
/// Only label add/remove effects contribute; non-label effects are ignored. The
/// order follows the plan order of `effects`, matching the operator-facing line.
/// Returns the empty string when no label effects were applied (the human
/// renderer omits an empty segment).
pub fn labels_delta(effects: &[WorkflowEffect]) -> String {
    let mut parts = Vec::new();
    for effect in effects {
        match effect {
            WorkflowEffect::AddLabel(label) => parts.push(format!("+{label}")),
            WorkflowEffect::RemoveLabel(label) => parts.push(format!("-{label}")),
            _ => {}
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::{ItemNumber, RepositoryId};
    use temper_log::ArtifactKind;
    use temper_workflow::{ArtifactKindId, ArtifactSource, LabelId, QueueId, RoleId};

    fn identity(source: ArtifactSource) -> WorkItemIdentity {
        WorkItemIdentity::new(
            &RepositoryId::new("forgejo:acme/widgets"),
            &RoleId::new("architect"),
            &QueueId::new("triage"),
            source,
            &ArtifactKindId::new("intake"),
        )
    }

    #[test]
    fn work_item_ref_strips_scheme_and_uses_issue_shape() {
        let r = work_item_ref(&identity(ArtifactSource::Issue {
            number: ItemNumber::new(42),
        }));
        assert_eq!(r.to_string(), "acme/widgets#42");
        assert_eq!(r.kind(), ArtifactKind::Issue);
    }

    #[test]
    fn work_item_ref_uses_pull_request_shape() {
        let r = work_item_ref(&identity(ArtifactSource::PullRequest {
            number: ItemNumber::new(44),
        }));
        assert_eq!(r.to_string(), "acme/widgets PR#44");
        assert_eq!(r.kind(), ArtifactKind::PullRequest);
    }

    #[test]
    fn labels_delta_keeps_plan_order_and_signs() {
        let effects = vec![
            WorkflowEffect::RemoveLabel(LabelId::new("untriaged")),
            WorkflowEffect::AddLabel(LabelId::new("code")),
            WorkflowEffect::AddLabel(LabelId::new("ready")),
        ];
        assert_eq!(labels_delta(&effects), "-untriaged +code +ready");
    }

    #[test]
    fn labels_delta_is_empty_without_label_effects() {
        assert_eq!(labels_delta(&[WorkflowEffect::MergePullRequest]), "");
    }
}
