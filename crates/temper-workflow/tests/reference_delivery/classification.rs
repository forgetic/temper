use super::*;

#[test]
fn reference_metadata_relations_classify_to_declared_kinds() {
    let workflow = fixture_workflow();
    let classifier = Classifier::new(&workflow);
    let code_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(2))],
        dependencies: vec![ArtifactRef::same_repo(ItemNumber::new(3))],
        ..WorkflowMetadata::default()
    });
    let code = classifier
        .classify_issue(&Issue {
            body: code_body,
            ..issue(1, &["code", "blocked"])
        })
        .expect("code issue with relations classifies");

    assert_eq!(
        code.relations,
        vec![
            ClassifiedRelation {
                kind: RelationKind::Parent,
                source: ArtifactKindId::new("code"),
                target: ArtifactRef::same_repo(ItemNumber::new(2)),
                target_kinds: vec![ArtifactKindId::new("design"), ArtifactKindId::new("epic")],
            },
            ClassifiedRelation {
                kind: RelationKind::Dependency,
                source: ArtifactKindId::new("code"),
                target: ArtifactRef::same_repo(ItemNumber::new(3)),
                target_kinds: vec![ArtifactKindId::new("code")],
            },
        ]
    );

    let pr_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(1))],
        ..WorkflowMetadata::default()
    });
    let pr = classifier
        .classify_pull_request(&PullRequest {
            body: pr_body,
            ..pull_request(4, &["implementation"])
        })
        .expect("implementation PR relation classifies");
    assert_eq!(
        pr.relations,
        vec![ClassifiedRelation {
            kind: RelationKind::ProducedPr,
            source: ArtifactKindId::new("implementation_pr"),
            target: ArtifactRef::same_repo(ItemNumber::new(1)),
            target_kinds: vec![ArtifactKindId::new("code")],
        }]
    );
}
