use super::*;

#[test]
fn unlabeled_issue_is_serviced_by_the_mechanical_intake_queue() {
    // A freshly filed human issue carries no labels at all. The default `intake`
    // kind admits it, and the empty-label `raw_intake` queue services it
    // mechanically, planning `mark_untriaged` to stamp the `untriaged` label.
    // This is intake flow step 1->2 (issue #35): unlabeled issue -> untriaged.
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &[]);
    let workflow = workflow_from_json(INTAKE_DEFAULT_FIXTURE);
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_automated_queues(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        vec![AutomatedWorkItem {
            queue: QueueId::new("raw_intake"),
            actor: RoleId::new("mechanical"),
            transition: temper_workflow::TransitionId::new("mark_untriaged"),
            executor: None,
            outcomes: std::collections::BTreeMap::new(),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}

#[test]
fn labeled_issue_is_not_serviced_by_the_default_intake_queue() {
    // A labeled issue classifies as its specific kind (`code`), not the default
    // catch-all `intake`. The empty-label intake queue selects only intake
    // artifacts, so a `code` issue is left for its own queues: the default kind
    // does not change behavior for labeled issues.
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let _ = create_issue(&forge, &repo, &["code"]);
    let workflow = workflow_from_json(INTAKE_DEFAULT_FIXTURE);
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_automated_queues(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        Vec::new(),
    );
}

#[test]
fn reference_fixture_services_raw_intake_via_mechanical_queue() {
    // End-to-end guard for the demo's first hop: in the *canonical*
    // reference-delivery workflow, a freshly filed unlabeled human issue is
    // admitted by the default `intake` kind and serviced by the label-less
    // `raw_intake` automation queue, which plans the mechanical `mark_untriaged`
    // stamp. Without this queue the issue never gains `untriaged`, the
    // architect's `design_triage` queue never matches, and the whole pipeline
    // stalls at issue #1.
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &[]);
    let workflow = workflow();
    let compiled = workflow.compile();

    assert_eq!(
        block_on(scan_automated_queues(
            &forge,
            &repo,
            &workflow,
            &compiled,
            ts("2026-05-29T00:00:00Z"),
        ))
        .expect("scan succeeds"),
        vec![AutomatedWorkItem {
            queue: QueueId::new("raw_intake"),
            actor: RoleId::new("mechanical"),
            transition: temper_workflow::TransitionId::new("mark_untriaged"),
            executor: None,
            outcomes: std::collections::BTreeMap::new(),
            target: ArtifactSource::Issue { number },
            kind: ArtifactKindId::new("intake"),
        }]
    );
}
