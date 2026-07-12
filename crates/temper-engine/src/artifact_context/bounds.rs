// SPDX-License-Identifier: MPL-2.0

use temper_protocol_worker::{
    ArtifactContextBundle, ArtifactContextDiagnosticCode, ArtifactReference,
};

use super::ArtifactContextPolicy;
use super::lineage::{diagnostic, key};

pub(super) fn enforce_bounds(
    bundle: &mut ArtifactContextBundle,
    mandatory_index: usize,
    policy: ArtifactContextPolicy,
) {
    for index in 0..bundle.snapshots.len() {
        if bundle.snapshots[index].body.len() <= policy.body_bytes {
            continue;
        }
        truncate_utf8(&mut bundle.snapshots[index].body, policy.body_bytes);
        let source = bundle.snapshots[index].artifact.clone();
        note_content_loss(
            bundle,
            "artifact body exceeded the per-body byte limit",
            Some(source),
        );
    }

    while serialized_len(bundle) > policy.bundle_bytes && bundle.index.len() > mandatory_index {
        let removed = bundle.index.pop().expect("optional index remains");
        let removed_key = key(&removed.artifact);
        bundle.relations.retain(|relation| {
            key(&relation.source) != removed_key && key(&relation.target) != removed_key
        });
        bundle.truncation.count_exceeded = true;
        bundle.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::CountExceeded,
            "optional summary dropped to satisfy serialized bundle limit",
            Some(removed.artifact),
        ));
    }

    while serialized_len(bundle) > policy.bundle_bytes {
        let Some(index) = bundle
            .snapshots
            .iter()
            .rposition(|snapshot| !snapshot.body.is_empty())
        else {
            break;
        };
        let excess = serialized_len(bundle).saturating_sub(policy.bundle_bytes);
        let current = bundle.snapshots[index].body.len();
        let target = current.saturating_sub(excess.max(1));
        truncate_utf8(&mut bundle.snapshots[index].body, target);
        let source = bundle.snapshots[index].artifact.clone();
        note_content_loss(
            bundle,
            "artifact body truncated to satisfy serialized bundle limit",
            Some(source),
        );
    }
}

fn note_content_loss(
    bundle: &mut ArtifactContextBundle,
    message: &str,
    source: Option<ArtifactReference>,
) {
    bundle.truncation.content_truncated = true;
    bundle.diagnostics.push(diagnostic(
        ArtifactContextDiagnosticCode::ContentTruncated,
        message,
        source,
    ));
}

fn truncate_utf8(value: &mut String, maximum: usize) {
    if value.len() <= maximum {
        return;
    }
    let mut boundary = maximum;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn serialized_len(bundle: &ArtifactContextBundle) -> usize {
    serde_json::to_vec(bundle)
        .expect("artifact context always serializes")
        .len()
}

#[cfg(test)]
mod tests {
    use temper_protocol_worker::{ArtifactRepository, ArtifactType};

    use super::*;

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        let mut value = "aa🦀bb".to_string();
        truncate_utf8(&mut value, 4);
        assert_eq!(value, "aa");
    }

    #[test]
    fn empty_bundle_is_below_default_limit() {
        let bundle = ArtifactContextBundle::new(
            ArtifactRepository {
                id: "1".into(),
                path: "a/b".into(),
            },
            ArtifactType::Issue,
        );
        assert!(serialized_len(&bundle) < ArtifactContextPolicy::default().bundle_bytes);
    }
}
