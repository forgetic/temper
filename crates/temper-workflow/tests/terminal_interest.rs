use temper_workflow::{
    ArtifactTarget, Diagnostic, RawArtifactKind, RawLabel, RawQueue, RawQueueLabelSet,
    RawWorkflowSpec, workflow_interest,
};

#[test]
fn terminal_interest_uses_only_explicit_positive_queue_evidence() {
    let json = r#"{
      "name": "terminal-interest",
      "labels": [
        {"id":"code"}, {"id":"ready"}, {"id":"planned"},
        {"id":"validated"}, {"id":"recovery"}, {"id":"excluded"},
        {"id":"gate-label"}
      ],
      "artifact_kinds": [
        {"id":"code", "target":"issue", "identifying_labels":["code"]}
      ],
      "state_dimensions": [{
        "id":"history", "states":[
          {"id":"planned", "label":"planned"},
          {"id":"validated", "label":"validated"}
        ]
      }],
      "queues": [
        {"id":"ordinary", "artifact":"code", "labels":["ready"]},
        {
          "id":"recover_terminal", "artifact":"code", "labels":["recovery"],
          "excluded_labels":["excluded"],
          "condition":{"kind":"label_present", "label":"gate-label"},
          "terminal":true
        }
      ],
      "transitions": [{
        "id":"record_validation", "artifact":"code",
        "effects":[{"kind":"add_label", "label":"validated"}]
      }]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow parses");
    let workflow = spec.validate().expect("workflow validates");
    let interest = workflow_interest(&workflow);

    assert_eq!(
        interest.terminal_labels(ArtifactTarget::Issue),
        &["recovery".to_string()]
    );
    for historical in [
        "code",
        "ready",
        "planned",
        "validated",
        "excluded",
        "gate-label",
    ] {
        assert!(
            !interest
                .terminal_labels(ArtifactTarget::Issue)
                .iter()
                .any(|label| label == historical),
            "{historical} must not become terminal interest implicitly"
        );
    }
    let compiled = workflow.compile();
    assert!(
        compiled
            .queues()
            .iter()
            .find(|queue| queue.id.as_str() == "recover_terminal")
            .expect("terminal queue compiled")
            .terminal
    );
}

#[test]
fn condition_only_terminal_queue_uses_identifying_label_fallback() {
    let json = r#"{
      "name":"condition-terminal",
      "labels":[{"id":"implementation"}],
      "artifact_kinds":[{
        "id":"implementation_pr", "target":"pull_request",
        "identifying_labels":["implementation"]
      }],
      "queues":[{
        "id":"ci_recovery", "artifact":"implementation_pr",
        "condition":{"kind":"ci_recovery_required"}, "terminal":true
      }]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow parses");
    let workflow = spec
        .validate()
        .expect("identified terminal queue validates");
    assert_eq!(
        workflow_interest(&workflow).terminal_labels(ArtifactTarget::PullRequest),
        &["implementation".to_string()]
    );
}

#[test]
fn unlabelled_terminal_queue_is_rejected() {
    let spec = RawWorkflowSpec {
        name: "invalid-terminal".to_string(),
        labels: vec![RawLabel {
            id: "ready".to_string(),
            description: None,
        }],
        artifact_kinds: vec![RawArtifactKind {
            id: "intake".to_string(),
            target: ArtifactTarget::Issue,
            identifying_labels: Vec::new(),
            initial_labels: Vec::new(),
        }],
        queues: vec![RawQueue {
            id: "all_terminal".to_string(),
            artifacts: vec!["intake".to_string()],
            terminal: true,
            any_of: vec![
                RawQueueLabelSet::default(),
                RawQueueLabelSet {
                    labels: vec!["ready".to_string()],
                },
            ],
            ..RawQueue::default()
        }],
        ..RawWorkflowSpec::default()
    };

    let errors = spec
        .validate()
        .expect_err("unfiltered terminal declaration must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UnfilteredTerminalQueue {
                queue: "all_terminal".to_string(),
                artifacts: vec!["intake".to_string()],
            })
    );
}
