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
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, GateSignals, QueueManifest, RoleId, WorkflowEffect,
};

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

/// The §7 `queue.entered` destination after a transition's label effects.
///
/// A transition that flips identifying labels moves the artifact into whichever
/// queue's required labels it now satisfies; §7 renders that as
/// `-> queue '<queue>' | awaiting <role>`. We derive that destination from the
/// labels the transition *added* (the `applied` effects): a queue is the
/// destination when it declares at least one required label and every one of
/// those labels is among the just-added set. The awaiting role is the queue's
/// first subscriber (queues list subscribers in role-declaration order).
///
/// This is a label-only heuristic — it does not re-read the artifact — so it
/// resolves the common single-/multi-label gated queues of §7 exactly while
/// staying a pure function of the workflow model and the applied effects.
/// When no queue matches (e.g. a transition that only sets a body or merges),
/// it returns `None` and the caller emits no `queue.entered` line.
pub fn queue_after_transition<'a>(
    compiled: &'a CompiledWorkflow,
    applied: &[WorkflowEffect],
) -> Option<(&'a QueueManifest, Option<&'a RoleId>)> {
    let added: std::collections::BTreeSet<&str> = applied
        .iter()
        .filter_map(|effect| match effect {
            WorkflowEffect::AddLabel(label) => Some(label.as_str()),
            _ => None,
        })
        .collect();
    if added.is_empty() {
        return None;
    }
    // Prefer the most specific match (most required labels) so a transition
    // adding several labels lands in the narrowest matching queue.
    compiled
        .queues()
        .iter()
        .filter(|queue| {
            !queue.labels.is_empty()
                && queue
                    .labels
                    .iter()
                    .all(|label| added.contains(label.as_str()))
        })
        .max_by_key(|queue| queue.labels.len())
        .map(|queue| (queue, queue.subscribers.first()))
}

/// Renders the §7 `gate.evaluated` gate summary from fresh signals.
///
/// Produces the `ci_gate=<state> dependency_gate=<state>` field the human line
/// carries (§7: `gates: ci_gate=pending dependency_gate=ok`). CI maps to
/// `ok`/`failed`/`pending`; the dependency gate is `ok` unless a fresh read of a
/// prerequisite failed (in which case it is `pending`, the conservative
/// not-yet-landed verdict). Only the two gates §7 shows are rendered.
pub fn gate_summary(signals: &GateSignals) -> String {
    let ci = if signals.ci().is_passed() {
        "ok"
    } else if signals.ci().is_failed() {
        "failed"
    } else {
        "pending"
    };
    let dependency = if signals.dependencies().read_failures().is_empty() {
        "ok"
    } else {
        "pending"
    };
    format!("ci_gate={ci} dependency_gate={dependency}")
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

    /// A §7-shaped workflow: a `code_ready` queue gated on `ready` and
    /// subscribed by `engineer`, plus a `landing` queue gated on `landed`.
    const QUEUE_WORKFLOW: &str = r#"{
        "name": "queue-derivation",
        "roles": [
            {"id": "architect", "queues": ["triage"]},
            {"id": "engineer", "queues": ["code_ready"]}
        ],
        "labels": [
            {"id": "untriaged"}, {"id": "code"}, {"id": "ready"}, {"id": "landed"}
        ],
        "artifact_kinds": [
            {"id": "intake", "target": "issue", "identifying_labels": ["untriaged"]},
            {"id": "code", "target": "issue", "identifying_labels": ["code"]}
        ],
        "queues": [
            {"id": "triage", "artifact": "intake", "labels": ["untriaged"]},
            {"id": "code_ready", "artifact": "code", "labels": ["ready"]},
            {"id": "landing", "artifact": "code", "labels": ["landed"]}
        ],
        "transitions": [
            {
                "id": "triage_intake_to_code",
                "artifact": "intake",
                "roles": ["architect"],
                "effects": [
                    {"kind": "remove_label", "label": "untriaged"},
                    {"kind": "add_label", "label": "code"},
                    {"kind": "add_label", "label": "ready"}
                ]
            }
        ]
    }"#;

    fn queue_workflow() -> temper_workflow::CompiledWorkflow {
        let spec: temper_workflow::RawWorkflowSpec =
            serde_json::from_str(QUEUE_WORKFLOW).expect("json parses");
        spec.validate().expect("workflow validates").compile()
    }

    #[test]
    fn queue_after_transition_picks_label_matched_queue_and_role() {
        let compiled = queue_workflow();
        // Adding `code` + `ready` lands in `code_ready` (labels=[ready]),
        // awaiting its subscriber `engineer` — exactly the §7 line.
        let applied = vec![
            WorkflowEffect::RemoveLabel(LabelId::new("untriaged")),
            WorkflowEffect::AddLabel(LabelId::new("code")),
            WorkflowEffect::AddLabel(LabelId::new("ready")),
        ];
        let (queue, role) =
            queue_after_transition(&compiled, &applied).expect("a destination queue matches");
        assert_eq!(queue.id.as_str(), "code_ready");
        assert_eq!(role.map(RoleId::as_str), Some("engineer"));
    }

    #[test]
    fn queue_after_transition_is_none_without_a_label_match() {
        let compiled = queue_workflow();
        // A merge effect adds no labels, so no queue is entered.
        assert!(queue_after_transition(&compiled, &[WorkflowEffect::MergePullRequest]).is_none());
        // Adding a label no queue requires also yields no destination.
        assert!(
            queue_after_transition(&compiled, &[WorkflowEffect::AddLabel(LabelId::new("code"))])
                .is_none()
        );
    }

    #[test]
    fn gate_summary_renders_ci_and_dependency_states() {
        use temper_workflow::{CiStatus, DependencyStatus};
        // Pending CI, clean dependency read -> the §7 `waiting on CI` shape.
        let pending = GateSignals::new();
        assert_eq!(gate_summary(&pending), "ci_gate=pending dependency_gate=ok");
        // Passed CI -> eligible-to-land shape.
        let passed = GateSignals::new().with_ci(CiStatus::passed());
        assert_eq!(gate_summary(&passed), "ci_gate=ok dependency_gate=ok");
        // Failed CI is distinguished from pending.
        let failed = GateSignals::new().with_ci(CiStatus::failed());
        assert_eq!(gate_summary(&failed), "ci_gate=failed dependency_gate=ok");
        // A dependency read failure degrades the dependency gate to pending.
        let mut deps = DependencyStatus::new();
        deps.mark_read_failure(temper_forge::ItemNumber::new(7), "boom");
        let dep_failed = GateSignals::new().with_dependencies(deps);
        assert_eq!(
            gate_summary(&dep_failed),
            "ci_gate=pending dependency_gate=pending"
        );
    }
}
