//! Validation tests for `temper-workflow` Phase 2 (spec + validation).

use temper_workflow::{
    ArtifactTarget, Diagnostic, IntakeAuthor, RawArtifactKind, RawEffect, RawGate,
    RawGateCondition, RawIntakeAuthor, RawLabel, RawQueue, RawQueueAction, RawQueueLabelSet,
    RawRelation, RawRole, RawState, RawStateDimension, RawTransition, RawWorkflowSpec,
    ReferenceSite, RelationKind, Severity, SymbolKind, ValidatedWorkflow,
};

#[path = "validation/basics.rs"]
mod basics;
#[path = "validation/default_artifacts.rs"]
mod default_artifacts;
#[path = "validation/intake_author.rs"]
mod intake_author;
#[path = "validation/queues.rs"]
mod queues;
#[path = "validation/references.rs"]
mod references;

/// Builds a small but complete workflow that exercises every reference kind:
/// role -> queue, queue -> artifact, queue -> label, artifact -> label,
/// relation -> artifact endpoints, state -> label,
/// transition -> artifact/role/gate/effect-label, and gate -> transition/condition.
fn valid_spec() -> RawWorkflowSpec {
    RawWorkflowSpec {
        name: "code-review".to_string(),
        labels: vec![
            RawLabel {
                id: "epic".to_string(),
                description: Some("identifies an epic artifact".to_string()),
            },
            RawLabel {
                id: "code".to_string(),
                description: Some("identifies a code artifact".to_string()),
            },
            RawLabel {
                id: "ready".to_string(),
                description: Some("ready to claim".to_string()),
            },
            RawLabel {
                id: "in-progress".to_string(),
                description: None,
            },
            RawLabel {
                id: "needs-review".to_string(),
                description: None,
            },
            RawLabel {
                id: "review-approved".to_string(),
                description: None,
            },
        ],
        roles: vec![
            RawRole {
                id: "engineer".to_string(),
                charter: Some("implements code issues".to_string()),
                prompt: Default::default(),
                external_tools: Vec::new(),
                concurrency: Some(2),
                queues: vec!["code_ready".to_string()],
            },
            RawRole {
                id: "reviewer".to_string(),
                charter: None,
                prompt: Default::default(),
                external_tools: Vec::new(),
                concurrency: None,
                queues: vec!["needs_review".to_string()],
            },
        ],
        artifact_kinds: vec![
            RawArtifactKind {
                id: "epic".to_string(),
                target: ArtifactTarget::Issue,
                identifying_labels: vec!["epic".to_string()],
                initial_labels: Vec::new(),
            },
            RawArtifactKind {
                id: "code".to_string(),
                target: ArtifactTarget::Issue,
                identifying_labels: vec!["code".to_string()],
                initial_labels: Vec::new(),
            },
        ],
        relations: vec![RawRelation {
            kind: RelationKind::Parent,
            source: "code".to_string(),
            target: "epic".to_string(),
        }],
        state_dimensions: vec![RawStateDimension {
            id: "code_lifecycle".to_string(),
            exclusive: true,
            states: vec![
                RawState {
                    id: "ready".to_string(),
                    label: Some("ready".to_string()),
                    artifacts: Vec::new(),
                },
                RawState {
                    id: "in_progress".to_string(),
                    label: Some("in-progress".to_string()),
                    artifacts: Vec::new(),
                },
            ],
        }],
        queues: vec![
            RawQueue {
                id: "code_ready".to_string(),
                artifacts: vec!["code".to_string()],
                labels: vec!["ready".to_string()],
                excluded_labels: Vec::new(),
                any_of: Vec::new(),
                min_depth: None,
                max_age: None,
                condition: None,
                automation: None,
                actions: Vec::new(),
            },
            RawQueue {
                id: "needs_review".to_string(),
                artifacts: vec!["code".to_string()],
                labels: vec!["needs-review".to_string()],
                excluded_labels: Vec::new(),
                any_of: Vec::new(),
                min_depth: None,
                max_age: None,
                condition: None,
                automation: None,
                actions: Vec::new(),
            },
        ],
        transitions: vec![
            RawTransition {
                id: "claim_code".to_string(),
                artifact: "code".to_string(),
                roles: vec!["engineer".to_string()],
                requires_gates: Vec::new(),
                effects: vec![
                    RawEffect::RemoveLabel {
                        label: "ready".to_string(),
                        if_present: false,
                    },
                    RawEffect::AddLabel {
                        label: "in-progress".to_string(),
                    },
                ],
                outcomes: Default::default(),
            },
            RawTransition {
                id: "approve_review".to_string(),
                artifact: "code".to_string(),
                roles: vec!["reviewer".to_string()],
                requires_gates: vec!["review_gate".to_string()],
                effects: vec![RawEffect::AddLabel {
                    label: "review-approved".to_string(),
                }],
                outcomes: Default::default(),
            },
        ],
        gates: vec![RawGate {
            id: "review_gate".to_string(),
            satisfied_by: vec!["approve_review".to_string()],
            condition: None,
        }],
        intake_author: None,
    }
}
