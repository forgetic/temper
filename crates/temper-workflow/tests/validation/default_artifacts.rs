use super::*;

#[test]
fn single_default_artifact_kind_validates() {
    let mut spec = valid_spec();
    // A single catch-all issue kind (no identifying labels) is the legal way to
    // model raw human intake.
    spec.artifact_kinds.push(RawArtifactKind {
        id: "intake".to_string(),
        target: ArtifactTarget::Issue,
        identifying_labels: Vec::new(),
        initial_labels: Vec::new(),
    });

    spec.validate()
        .expect("one default kind per target validates");
}

#[test]
fn multiple_default_artifact_kinds_for_one_target_is_diagnosed() {
    let mut spec = valid_spec();
    // Two catch-all issue kinds make classification of an unlabeled issue
    // ambiguous; validation must reject the pair.
    spec.artifact_kinds.push(RawArtifactKind {
        id: "intake".to_string(),
        target: ArtifactTarget::Issue,
        identifying_labels: Vec::new(),
        initial_labels: Vec::new(),
    });
    spec.artifact_kinds.push(RawArtifactKind {
        id: "inbox".to_string(),
        target: ArtifactTarget::Issue,
        identifying_labels: Vec::new(),
        initial_labels: Vec::new(),
    });

    let errors = spec
        .validate()
        .expect_err("two default kinds for one target must fail");
    let diagnostic = errors
        .diagnostics()
        .iter()
        .find_map(|diagnostic| match diagnostic {
            Diagnostic::MultipleDefaultArtifactKinds { target, kinds } => Some((target, kinds)),
            _ => None,
        })
        .expect("the multiple-default diagnostic is reported");
    assert_eq!(diagnostic.0, "issue");
    assert!(diagnostic.1.contains(&"intake".to_string()));
    assert!(diagnostic.1.contains(&"inbox".to_string()));
}

#[test]
fn one_default_per_distinct_target_validates() {
    let mut spec = valid_spec();
    // A default kind per target is fine: the constraint is one-per-target, not
    // one-per-workflow.
    spec.artifact_kinds.push(RawArtifactKind {
        id: "intake".to_string(),
        target: ArtifactTarget::Issue,
        identifying_labels: Vec::new(),
        initial_labels: Vec::new(),
    });
    spec.artifact_kinds.push(RawArtifactKind {
        id: "pr_intake".to_string(),
        target: ArtifactTarget::PullRequest,
        identifying_labels: Vec::new(),
        initial_labels: Vec::new(),
    });

    spec.validate()
        .expect("one default per distinct target validates");
}
