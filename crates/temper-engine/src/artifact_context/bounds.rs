// SPDX-License-Identifier: MPL-2.0

use temper_protocol_worker::{
    ArtifactContextBundle, ArtifactContextDiagnosticCode, ArtifactReference,
};

use super::ArtifactContextPolicy;
use super::lineage::diagnostic;

pub(super) fn enforce_bounds(bundle: &mut ArtifactContextBundle, policy: ArtifactContextPolicy) {
    if bundle.primary.body.len() > policy.body_bytes {
        truncate_utf8(&mut bundle.primary.body, policy.body_bytes);
        let source = bundle.primary.artifact.clone();
        note_content_loss(
            bundle,
            "artifact body exceeded the per-body byte limit",
            Some(source),
        );
    }
    for index in 0..bundle.lineage.len() {
        if bundle.lineage[index].body.len() <= policy.body_bytes {
            continue;
        }
        truncate_utf8(&mut bundle.lineage[index].body, policy.body_bytes);
        let source = bundle.lineage[index].artifact.clone();
        note_content_loss(
            bundle,
            "artifact body exceeded the per-body byte limit",
            Some(source),
        );
    }

    while serialized_len(bundle) > policy.bundle_bytes {
        let removed = bundle
            .optional_references
            .pop()
            .or_else(|| bundle.validation_scope.pop());
        let Some(removed) = removed else {
            break;
        };
        bundle.truncation.count_exceeded = true;
        bundle.diagnostics.push(diagnostic(
            ArtifactContextDiagnosticCode::CountExceeded,
            "optional summary dropped to satisfy serialized bundle limit",
            Some(removed.artifact),
        ));
    }

    if serialized_len(bundle) > policy.bundle_bytes {
        let source = bundle
            .lineage
            .iter()
            .rfind(|snapshot| !snapshot.body.is_empty())
            .map(|snapshot| snapshot.artifact.clone())
            .or_else(|| (!bundle.primary.body.is_empty()).then(|| bundle.primary.artifact.clone()));
        if source.is_some() {
            note_content_loss(
                bundle,
                "artifact body truncated to satisfy serialized bundle limit",
                source,
            );
        }
    }
    while serialized_len(bundle) > policy.bundle_bytes {
        let lineage_index = bundle
            .lineage
            .iter()
            .rposition(|snapshot| !snapshot.body.is_empty());
        let current = match lineage_index {
            Some(index) => bundle.lineage[index].body.len(),
            None if !bundle.primary.body.is_empty() => bundle.primary.body.len(),
            None => break,
        };
        let excess = serialized_len(bundle).saturating_sub(policy.bundle_bytes);
        let target = current.saturating_sub(excess.max(1));
        match lineage_index {
            Some(index) => truncate_utf8(&mut bundle.lineage[index].body, target),
            None => truncate_utf8(&mut bundle.primary.body, target),
        }
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
    use temper_protocol_worker::{ArtifactRepository, ArtifactSnapshot, ArtifactType};

    use super::*;

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        let mut value = "aa🦀bb".to_string();
        truncate_utf8(&mut value, 4);
        assert_eq!(value, "aa");
    }

    #[test]
    fn per_body_bounds_truncate_primary_and_lineage_on_utf8_boundaries() {
        let repository = ArtifactRepository {
            id: "1".into(),
            path: "a/b".into(),
        };
        let mut bundle = ArtifactContextBundle::new(ArtifactSnapshot {
            artifact: ArtifactReference {
                repository: repository.clone(),
                artifact_type: ArtifactType::Issue,
                number: 2,
            },
            title: "primary".into(),
            body: "🦀".repeat(10),
            labels: Vec::new(),
            state: "open".into(),
            workflow_kind: Some("code".into()),
            workflow: None,
        });
        bundle.lineage.push(ArtifactSnapshot {
            artifact: ArtifactReference {
                repository,
                artifact_type: ArtifactType::Issue,
                number: 1,
            },
            title: "ancestor".into(),
            body: "🦀".repeat(10),
            labels: Vec::new(),
            state: "open".into(),
            workflow_kind: Some("feature".into()),
            workflow: None,
        });

        enforce_bounds(
            &mut bundle,
            ArtifactContextPolicy {
                body_bytes: 5,
                ..ArtifactContextPolicy::default()
            },
        );

        assert_eq!(bundle.primary.body, "🦀");
        assert_eq!(bundle.lineage[0].body, "🦀");
        assert!(bundle.truncation.content_truncated);
    }

    #[test]
    fn serialized_size_bound_truncates_bodies_deterministically() {
        let mut bundle = ArtifactContextBundle::new(ArtifactSnapshot {
            artifact: ArtifactReference {
                repository: ArtifactRepository {
                    id: "1".into(),
                    path: "a/b".into(),
                },
                artifact_type: ArtifactType::Issue,
                number: 1,
            },
            title: "primary".into(),
            body: "a".repeat(10_000),
            labels: Vec::new(),
            state: "open".into(),
            workflow_kind: Some("code".into()),
            workflow: None,
        });

        enforce_bounds(
            &mut bundle,
            ArtifactContextPolicy {
                body_bytes: 20_000,
                bundle_bytes: 2_000,
                ..ArtifactContextPolicy::default()
            },
        );

        assert!(serialized_len(&bundle) <= 2_000);
        assert!(bundle.truncation.content_truncated);
    }

    #[test]
    fn minimal_bundle_is_below_default_limit() {
        let bundle = ArtifactContextBundle::new(ArtifactSnapshot {
            artifact: ArtifactReference {
                repository: ArtifactRepository {
                    id: "1".into(),
                    path: "a/b".into(),
                },
                artifact_type: ArtifactType::Issue,
                number: 1,
            },
            title: "primary".into(),
            body: String::new(),
            labels: Vec::new(),
            state: "open".into(),
            workflow_kind: None,
            workflow: None,
        });
        assert!(serialized_len(&bundle) < ArtifactContextPolicy::default().bundle_bytes);
    }
}
