use super::*;

#[test]
fn multi_artifact_disjunctive_queue_validates() {
    let mut spec = valid_spec();
    spec.queues.push(RawQueue {
        id: "mixed_return".to_string(),
        artifacts: vec!["epic".to_string(), "code".to_string()],
        labels: Vec::new(),
        excluded_labels: Vec::new(),
        any_of: vec![
            RawQueueLabelSet {
                labels: vec!["ready".to_string()],
            },
            RawQueueLabelSet {
                labels: vec!["needs-review".to_string()],
            },
        ],
        terminal: false,
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

#[test]
fn recovery_required_action_requires_read_only_pr_checkout_and_verdicts() {
    let mut spec = valid_spec();
    spec.queues[0].condition = Some(RawGateCondition::CiRecoveryRequired);
    spec.queues[0].actions.push(RawQueueAction {
        role: "engineer".to_string(),
        action: "claim_code".to_string(),
        checkout: Some("pull_request_writable".to_string()),
        ..RawQueueAction::default()
    });

    let errors = spec
        .validate()
        .expect_err("recovery action must be a verdict-driven read-only diagnostic");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueActionUnsafeCiRecoveryCheckout {
                queue: "code_ready".to_string(),
                role: "engineer".to_string(),
                action: "claim_code".to_string(),
                checkout: Some("pull_request_writable".to_string()),
            })
    );
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueActionCiRecoveryMissingOutcomes {
                queue: "code_ready".to_string(),
                role: "engineer".to_string(),
                action: "claim_code".to_string(),
            })
    );
}

#[test]
fn pull_request_writable_action_rejects_non_publishable_effects() {
    let mut spec = valid_spec();
    spec.artifact_kinds[1].target = ArtifactTarget::PullRequest;
    spec.transitions[0].effects = vec![RawEffect::CreateComment {
        body: "repair requested".to_string(),
    }];
    spec.queues[0].actions.push(RawQueueAction {
        role: "engineer".to_string(),
        action: "claim_code".to_string(),
        checkout: Some("pull_request_writable".to_string()),
        ..RawQueueAction::default()
    });

    let errors = spec
        .validate()
        .expect_err("non-publishable PR repair effect must fail validation");
    assert!(errors.diagnostics().contains(
        &Diagnostic::QueueActionUnsupportedPullRequestRepairEffect {
            queue: "code_ready".to_string(),
            role: "engineer".to_string(),
            action: "claim_code".to_string(),
            effect: "create_comment".to_string(),
        }
    ));
}

#[test]
fn pull_request_writable_action_accepts_publishable_effects() {
    let mut spec = valid_spec();
    spec.artifact_kinds[1].target = ArtifactTarget::PullRequest;
    spec.transitions[0].effects = vec![
        RawEffect::RemoveLabel {
            label: "ready".to_string(),
            if_present: true,
        },
        RawEffect::AddLabel {
            label: "in-progress".to_string(),
        },
        RawEffect::SetAssignee {
            role: "engineer".to_string(),
        },
        RawEffect::RemoveAssignee {
            role: "reviewer".to_string(),
        },
        RawEffect::RequestReviewers {
            roles: vec!["reviewer".to_string()],
        },
    ];
    spec.queues[0].actions.push(RawQueueAction {
        role: "engineer".to_string(),
        action: "claim_code".to_string(),
        checkout: Some("pull_request_writable".to_string()),
        ..RawQueueAction::default()
    });

    spec.validate()
        .expect("supported PR repair effects must validate");
}
