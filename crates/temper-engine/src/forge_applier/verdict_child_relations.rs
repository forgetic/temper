// SPDX-License-Identifier: MPL-2.0

//! Relation-aware validation for worker-authored `create_issues` children.

use std::collections::BTreeSet;

use temper_forge::ItemNumber;
use temper_protocol_worker::JobChild;
use temper_workflow::{
    ArtifactKindId, ArtifactTarget, Effect, LabelId, RelationKind, TransitionId, ValidatedWorkflow,
};

use crate::InFlightJob;

pub(super) fn source_parent_kinds_after_transition(
    workflow: &ValidatedWorkflow,
    source_kind: &ArtifactKindId,
    source_labels: &[String],
    transition_id: &TransitionId,
) -> BTreeSet<ArtifactKindId> {
    let mut source_kinds = BTreeSet::from([source_kind.clone()]);
    let labels = projected_source_labels(workflow, source_labels, transition_id);

    for kind in workflow.artifact_kinds() {
        if kind.target != ArtifactTarget::Issue || kind.identifying_labels.is_empty() {
            continue;
        }
        if kind
            .identifying_labels
            .iter()
            .all(|label| labels.contains(label))
        {
            source_kinds.insert(kind.id.clone());
        }
    }

    source_kinds
}

pub(super) fn validate_child_parent_relation(
    workflow: &ValidatedWorkflow,
    job: &InFlightJob,
    number: ItemNumber,
    child: &JobChild,
    child_kind: &ArtifactKindId,
    source_parent_kinds: &BTreeSet<ArtifactKindId>,
) -> bool {
    let declared_parent_targets = workflow
        .relations()
        .iter()
        .filter(|relation| relation.kind == RelationKind::Parent && relation.source == *child_kind)
        .map(|relation| relation.target.clone())
        .collect::<Vec<_>>();

    if declared_parent_targets.is_empty()
        || declared_parent_targets
            .iter()
            .any(|target| source_parent_kinds.contains(target))
    {
        return true;
    }

    tracing::warn!(
        target: "temper_daemon",
        job_id = %job.job_id,
        repo = %job.repo,
        issue = %number,
        child_slug = %child.slug,
        child_kind = %child_kind,
        allowed_parent_kinds = %join_kinds(&declared_parent_targets),
        source_parent_kinds = %join_kinds(source_parent_kinds),
        "forge applier dropped verdict apply with child artifact kind not allowed under source artifact kind"
    );
    false
}

fn projected_source_labels(
    workflow: &ValidatedWorkflow,
    source_labels: &[String],
    transition_id: &TransitionId,
) -> BTreeSet<LabelId> {
    let mut labels = source_labels
        .iter()
        .map(|label| LabelId::new(label.as_str()))
        .collect::<BTreeSet<_>>();

    let Some(transition) = workflow
        .transitions()
        .iter()
        .find(|transition| &transition.id == transition_id)
    else {
        return labels;
    };

    for effect in &transition.effects {
        match effect {
            Effect::AddLabel(label) => {
                labels.insert(label.clone());
            }
            Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
                labels.remove(label);
            }
            _ => {}
        }
    }

    labels
}

fn join_kinds<'a>(kinds: impl IntoIterator<Item = &'a ArtifactKindId>) -> String {
    kinds
        .into_iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
