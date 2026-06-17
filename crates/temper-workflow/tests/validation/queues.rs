use super::*;

#[test]
fn multi_artifact_disjunctive_queue_validates() {
    let mut spec = valid_spec();
    spec.queues.push(RawQueue {
        id: "mixed_return".to_string(),
        artifacts: vec!["epic".to_string(), "code".to_string()],
        labels: Vec::new(),
        any_of: vec![
            RawQueueLabelSet {
                labels: vec!["ready".to_string()],
            },
            RawQueueLabelSet {
                labels: vec!["needs-review".to_string()],
            },
        ],
        min_depth: None,
        max_age: None,
        condition: None,
        automation: None,
        actions: Vec::new(),
    });

    let workflow = spec.validate().expect("multi-kind OR queue validates");
    let queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "mixed_return")
        .expect("queue exists");
    assert_eq!(queue.artifacts.len(), 2);
    assert_eq!(queue.any_of.len(), 2);
}

#[test]
fn empty_queue_artifacts_are_diagnosed() {
    let mut spec = valid_spec();
    spec.queues[0].artifacts.clear();

    let errors = spec.validate().expect_err("empty artifact list must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::EmptyQueueArtifacts {
                queue: "code_ready".to_string(),
            })
    );
}

#[test]
fn queue_action_contract_is_validated() {
    let mut spec = valid_spec();
    spec.queues[0].actions.push(RawQueueAction {
        role: "reviewer".to_string(),
        action: "claim_code".to_string(),
        checkout: Some("sideways".to_string()),
        ..RawQueueAction::default()
    });

    let errors = spec
        .validate()
        .expect_err("unauthorized action assignment must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueActionUnauthorized {
                queue: "code_ready".to_string(),
                role: "reviewer".to_string(),
                action: "claim_code".to_string(),
            })
    );
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueActionInvalidCheckout {
                queue: "code_ready".to_string(),
                role: "reviewer".to_string(),
                action: "claim_code".to_string(),
                checkout: "sideways".to_string(),
            })
    );
}
