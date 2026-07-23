use super::*;

#[test]
fn explicit_kind_disagreement_quarantines_losslessly_with_one_bounded_audit() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (assignment, mut metadata) = expired_assignment("job-disagreement", "2026-05-29T00:10:00Z");
    metadata.parents = vec![ArtifactRef::same_repo(ItemNumber::new(11))];
    metadata.dependencies = vec![ArtifactRef::same_repo(ItemNumber::new(12))];
    metadata.correlation_key = Some("durable-correlation".to_string());
    metadata.target_branch = Some("feature/durable-recovery".to_string());
    metadata.repaired_head = Some("abc123".to_string());
    metadata.staged = true;
    metadata.create_issue_intents.insert(
        "durable-intent".to_string(),
        CreateIssuesIntent {
            transition: "create_children".to_string(),
            effect_index: 2,
            correlation_key: "children-correlation".to_string(),
            record_parent_dependencies: true,
            children: Vec::new(),
            completion: None,
            parent_wired: true,
            completed: false,
        },
    );
    let original_metadata = metadata.clone();
    let authored_prose = "Operator-authored context that quarantine must retain.";
    let body = format!(
        "{authored_prose}\n\n{}\n\nAuthored tail.",
        render_metadata_block(&metadata)
    );
    let mut labels = vec![
        "design".to_string(),
        "in-progress".to_string(),
        "priority-high".to_string(),
    ];
    labels.extend((0..100).map(|index| format!("unrelated-{index}")));
    let issue = create_claimed_issue_with(&forge, &repo, body, labels);
    let workflow = workflow();
    let converger =
        AssignmentConverger::new(&workflow, &forge, LeasePolicy::new(Duration::minutes(30)));
    let source = temper_workflow::ArtifactSource::Issue {
        number: issue.number,
    };

    assert_eq!(
        block_on(converger.validate_current(&repo, source, &assignment)).unwrap(),
        AssignmentValidation::Quarantined
    );
    block_on(converger.quarantine_target(&repo, source, "replayed stale diagnosis")).unwrap();

    let quarantined = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(quarantined.body.starts_with(authored_prose));
    assert!(quarantined.body.ends_with("Authored tail."));
    assert!(quarantined.labels.contains(&"needs-human".to_string()));
    assert!(quarantined.labels.contains(&"priority-high".to_string()));
    assert!(quarantined.labels.contains(&"unrelated-99".to_string()));
    assert!(!quarantined.labels.contains(&"in-progress".to_string()));
    let mut expected_metadata = original_metadata;
    expected_metadata.assignment = None;
    expected_metadata.lease = None;
    assert_eq!(
        parse_metadata_block(&quarantined.body).unwrap().unwrap(),
        expected_metadata
    );

    let comments = block_on(forge.list_issue_comments(&quarantined.id)).unwrap();
    assert_eq!(comments.len(), 1);
    let audit = &comments[0].body;
    assert!(audit.contains("Metadata kind: present (`code`)"));
    assert!(audit.contains("Relevant identifying labels: `design`"));
    assert!(audit.contains("Label-derived kind candidates: `design`"));
    assert!(audit.contains(temper_workflow::ASSIGNMENT_RECOVERY_AUDIT_MARKER));
    assert!(!audit.contains(authored_prose));
    assert!(!audit.contains("unrelated-99"));
    assert!(audit.chars().count() < 2_048);
}

#[test]
fn ambiguous_label_kind_quarantines_once_with_stable_candidates() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (assignment, mut metadata) = expired_assignment("job-ambiguous", "2026-05-29T00:10:00Z");
    metadata.kind = None;
    let issue = create_claimed_issue_with(
        &forge,
        &repo,
        render_metadata_block(&metadata),
        vec![
            "variant-b".to_string(),
            "code".to_string(),
            "in-progress".to_string(),
            "variant-a".to_string(),
        ],
    );
    let workflow = ambiguous_workflow();
    let converger =
        AssignmentConverger::new(&workflow, &forge, LeasePolicy::new(Duration::minutes(30)));
    let source = temper_workflow::ArtifactSource::Issue {
        number: issue.number,
    };

    assert_eq!(
        block_on(converger.validate_current(&repo, source, &assignment)).unwrap(),
        AssignmentValidation::Quarantined
    );
    block_on(converger.quarantine_target(&repo, source, "replayed ambiguity")).unwrap();

    let quarantined = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(quarantined.labels.contains(&"needs-human".to_string()));
    assert!(!quarantined.labels.contains(&"in-progress".to_string()));
    let comments = block_on(forge.list_issue_comments(&quarantined.id)).unwrap();
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0]
            .body
            .contains("Relevant identifying labels: `code`, `variant-a`, `variant-b`")
    );
    assert!(
        comments[0]
            .body
            .contains("Label-derived kind candidates: `code_variant_a`, `code_variant_b`")
    );
    assert!(comments[0].body.contains("Metadata kind: absent"));
}

#[test]
fn stale_target_finding_cannot_park_a_replacement_assignment() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let issue = create_claimed_issue_with(
        &forge,
        &repo,
        "<!-- temper:workflow\n{not-json}\n-->".to_string(),
        vec!["code".to_string(), "in-progress".to_string()],
    );
    let (replacement, replacement_metadata) =
        expired_assignment("job-replacement", "2026-05-29T00:40:00Z");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            body: Some(render_metadata_block(&replacement_metadata)),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let workflow = workflow();
    let converger =
        AssignmentConverger::new(&workflow, &forge, LeasePolicy::new(Duration::minutes(30)));
    block_on(converger.quarantine_target(
        &repo,
        temper_workflow::ArtifactSource::Issue {
            number: issue.number,
        },
        "stale malformed-metadata finding",
    ))
    .unwrap();

    let current = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(!current.labels.contains(&"needs-human".to_string()));
    assert_eq!(
        parse_metadata_block(&current.body)
            .unwrap()
            .unwrap()
            .assignment,
        Some(replacement)
    );
    assert!(
        block_on(forge.list_issue_comments(&current.id))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stale_invalid_finding_cannot_quarantine_a_renewed_assignment_snapshot() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let (mut stale_assignment, mut metadata) =
        expired_assignment("job-renewed", "2026-05-29T00:10:00Z");
    stale_assignment.queue = Some("missing_queue".to_string());
    metadata.assignment = Some(stale_assignment.clone());
    let issue = create_claimed_issue(&forge, &repo, &metadata);

    let mut renewed_assignment = stale_assignment.clone();
    renewed_assignment.expires_at = Some(ts("2026-05-29T00:40:00Z"));
    let mut renewed_metadata = metadata;
    renewed_metadata.assignment = Some(renewed_assignment.clone());
    let renewed_lease = renewed_metadata.lease.as_mut().unwrap();
    renewed_lease.heartbeat_at = ts("2026-05-29T00:20:00Z");
    renewed_lease.expires_at = ts("2026-05-29T00:40:00Z");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            body: Some(render_metadata_block(&renewed_metadata)),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let workflow = workflow();
    let converger =
        AssignmentConverger::new(&workflow, &forge, LeasePolicy::new(Duration::minutes(30)));
    assert_eq!(
        block_on(converger.validate_current(
            &repo,
            temper_workflow::ArtifactSource::Issue {
                number: issue.number,
            },
            &stale_assignment,
        ))
        .unwrap(),
        AssignmentValidation::Stale
    );

    let current = block_on(forge.get_issue_by_number(&repo, issue.number))
        .unwrap()
        .unwrap();
    assert!(!current.labels.contains(&"needs-human".to_string()));
    let current_metadata = parse_metadata_block(&current.body).unwrap().unwrap();
    assert_eq!(current_metadata.assignment, Some(renewed_assignment));
    assert_eq!(current_metadata.lease, renewed_metadata.lease);
    assert!(
        block_on(forge.list_issue_comments(&current.id))
            .unwrap()
            .is_empty()
    );
}
