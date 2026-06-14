//! Tests for queue automation metadata and validation.

use std::collections::BTreeMap;
use temper_workflow::{
    ArtifactTarget, Diagnostic, QueueAutomation, RawArtifactKind, RawExternalTool, RawLabel,
    RawQueue, RawQueueAutomation, RawRole, RawTransition, RawWorkflowSpec, ReferenceSite, RoleId,
    SymbolKind, TransitionId, VerdictId,
};

fn role(id: &str) -> RawRole {
    RawRole {
        id: id.to_string(),
        ..RawRole::default()
    }
}

fn automated_spec() -> RawWorkflowSpec {
    RawWorkflowSpec {
        name: "automated-landing".to_string(),
        roles: vec![role("mechanical")],
        labels: vec![
            RawLabel {
                id: "implementation-pr".to_string(),
                description: None,
            },
            RawLabel {
                id: "landing".to_string(),
                description: None,
            },
        ],
        artifact_kinds: vec![RawArtifactKind {
            id: "implementation_pr".to_string(),
            target: ArtifactTarget::PullRequest,
            identifying_labels: vec!["implementation-pr".to_string()],
            initial_labels: Vec::new(),
        }],
        queues: vec![RawQueue {
            id: "landing".to_string(),
            artifacts: vec!["implementation_pr".to_string()],
            labels: vec!["landing".to_string()],
            automation: Some(RawQueueAutomation {
                actor: "mechanical".to_string(),
                transition: "land_pr".to_string(),
                on_merge_conflict: Some("route_merge_conflict".to_string()),
                outcomes: BTreeMap::new(),
                ..RawQueueAutomation::default()
            }),
            ..RawQueue::default()
        }],
        transitions: vec![
            RawTransition {
                id: "land_pr".to_string(),
                artifact: "implementation_pr".to_string(),
                roles: vec!["mechanical".to_string()],
                ..RawTransition::default()
            },
            RawTransition {
                id: "route_merge_conflict".to_string(),
                artifact: "implementation_pr".to_string(),
                roles: vec!["mechanical".to_string()],
                ..RawTransition::default()
            },
        ],
        ..RawWorkflowSpec::default()
    }
}

fn automation_mut(spec: &mut RawWorkflowSpec) -> &mut RawQueueAutomation {
    spec.queues[0]
        .automation
        .as_mut()
        .expect("helper spec has queue automation")
}

fn add_issue_artifact_kind(spec: &mut RawWorkflowSpec) {
    spec.labels.push(RawLabel {
        id: "issue".to_string(),
        description: None,
    });
    spec.artifact_kinds.push(RawArtifactKind {
        id: "work_item".to_string(),
        target: ArtifactTarget::Issue,
        identifying_labels: vec!["issue".to_string()],
        initial_labels: Vec::new(),
    });
}

#[test]
fn raw_spec_loads_queue_automation_block() {
    let json = r#"{
        "name": "from-json",
        "roles": [{"id": "mechanical"}],
        "labels": [{"id": "implementation-pr"}, {"id": "landing"}],
        "artifact_kinds": [{
            "id": "implementation_pr",
            "target": "pull_request",
            "identifying_labels": ["implementation-pr"]
        }],
        "queues": [{
            "id": "landing",
            "artifact": "implementation_pr",
            "labels": ["landing"],
            "automation": {
                "actor": "mechanical",
                "transition": "land_pr",
                "on_merge_conflict": "route_merge_conflict"
            }
        }],
        "transitions": [
            {"id": "land_pr", "artifact": "implementation_pr", "roles": ["mechanical"]},
            {"id": "route_merge_conflict", "artifact": "implementation_pr", "roles": ["mechanical"]}
        ]
    }"#;

    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("json parses");
    assert_eq!(
        spec.queues[0].automation,
        Some(RawQueueAutomation {
            actor: "mechanical".to_string(),
            transition: "land_pr".to_string(),
            on_merge_conflict: Some("route_merge_conflict".to_string()),
            outcomes: BTreeMap::new(),
            ..RawQueueAutomation::default()
        })
    );
}

#[test]
fn duplicate_queue_automation_blocks_are_rejected_by_serde() {
    let json = r#"{
        "name": "duplicate-automation",
        "queues": [{
            "id": "landing",
            "artifact": "implementation_pr",
            "automation": {"actor": "mechanical", "transition": "land_pr"},
            "automation": {"actor": "mechanical", "transition": "other"}
        }]
    }"#;

    let error = serde_json::from_str::<RawWorkflowSpec>(json)
        .expect_err("duplicate automation field must fail deserialization");
    assert!(
        error.to_string().contains("duplicate field `automation`"),
        "unexpected error: {error}"
    );
}

#[test]
fn minimal_automated_queue_validates() {
    let workflow = automated_spec().validate().expect("automation validates");
    let queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "landing")
        .expect("queue exists");

    assert_eq!(
        queue.automation,
        Some(QueueAutomation {
            actor: RoleId::new("mechanical"),
            transition: TransitionId::new("land_pr"),
            executor: None,
            outcomes: BTreeMap::from([(
                VerdictId::merge_conflict(),
                TransitionId::new("route_merge_conflict"),
            )]),
        })
    );
}

#[test]
fn unknown_automation_actor_is_diagnosed() {
    let mut spec = automated_spec();
    automation_mut(&mut spec).actor = "ghost".to_string();

    let errors = spec.validate().expect_err("unknown actor fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Role,
                id: "ghost".to_string(),
                site: ReferenceSite::QueueAutomationActor {
                    queue: "landing".to_string(),
                },
            })
    );
}

#[test]
fn unknown_automation_transition_is_diagnosed() {
    let mut spec = automated_spec();
    automation_mut(&mut spec).transition = "missing".to_string();

    let errors = spec.validate().expect_err("unknown transition fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Transition,
                id: "missing".to_string(),
                site: ReferenceSite::QueueAutomationTransition {
                    queue: "landing".to_string(),
                },
            })
    );
}

#[test]
fn unauthorized_automation_transition_is_diagnosed() {
    let mut spec = automated_spec();
    spec.transitions[0].roles.clear();

    let errors = spec.validate().expect_err("unauthorized transition fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationUnauthorized {
                queue: "landing".to_string(),
                actor: "mechanical".to_string(),
                transition: "land_pr".to_string(),
            })
    );
}

#[test]
fn automation_transition_artifact_mismatch_is_diagnosed() {
    let mut spec = automated_spec();
    add_issue_artifact_kind(&mut spec);
    automation_mut(&mut spec).on_merge_conflict = None;
    spec.transitions[0].artifact = "work_item".to_string();

    let errors = spec.validate().expect_err("artifact mismatch fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationArtifactMismatch {
                queue: "landing".to_string(),
                transition: "land_pr".to_string(),
                artifact: "work_item".to_string(),
                queue_artifacts: vec!["implementation_pr".to_string()],
            })
    );
}

/// Declares an external tool with `tool_id` on the actor role of the helper
/// spec, so a workspace-backed automation can reference it as its `executor`.
fn declare_actor_external_tool(spec: &mut RawWorkflowSpec, tool_id: &str) {
    spec.roles[0].external_tools.push(RawExternalTool {
        id: tool_id.to_string(),
        description: "workspace executor".to_string(),
        ..RawExternalTool::default()
    });
}

#[test]
fn automation_executor_undeclared_on_actor_is_diagnosed() {
    let mut spec = automated_spec();
    // The automation names a workspace executor that the actor role never
    // declares among its external tools.
    automation_mut(&mut spec).executor = Some("coding_workspace".to_string());

    let errors = spec.validate().expect_err("undeclared executor fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationExecutorUndeclared {
                queue: "landing".to_string(),
                actor: "mechanical".to_string(),
                executor: "coding_workspace".to_string(),
            })
    );
}

#[test]
fn automation_executor_declared_on_actor_validates() {
    let mut spec = automated_spec();
    declare_actor_external_tool(&mut spec, "coding_workspace");
    automation_mut(&mut spec).executor = Some("coding_workspace".to_string());

    let workflow = spec.validate().expect("declared executor validates");
    let automation = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "landing")
        .and_then(|queue| queue.automation.clone())
        .expect("landing queue has automation");
    assert_eq!(
        automation.executor.as_ref().map(|id| id.as_str()),
        Some("coding_workspace"),
    );
}

#[test]
fn invalid_conflict_fallback_is_diagnosed() {
    let mut spec = automated_spec();
    add_issue_artifact_kind(&mut spec);
    spec.transitions[1].artifact = "work_item".to_string();
    spec.transitions[1].roles.clear();

    let errors = spec.validate().expect_err("invalid fallback fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationOutcomeUnauthorized {
                queue: "landing".to_string(),
                verdict: "merge_conflict".to_string(),
                actor: "mechanical".to_string(),
                transition: "route_merge_conflict".to_string(),
            })
    );
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationOutcomeArtifactMismatch {
                queue: "landing".to_string(),
                verdict: "merge_conflict".to_string(),
                transition: "route_merge_conflict".to_string(),
                expected: "implementation_pr".to_string(),
                actual: "work_item".to_string(),
            })
    );
}

#[test]
fn unknown_conflict_fallback_is_diagnosed() {
    let mut spec = automated_spec();
    automation_mut(&mut spec).on_merge_conflict = Some("missing_fallback".to_string());

    let errors = spec.validate().expect_err("unknown fallback fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Transition,
                id: "missing_fallback".to_string(),
                site: ReferenceSite::QueueAutomationOutcome {
                    queue: "landing".to_string(),
                    verdict: "merge_conflict".to_string(),
                },
            })
    );
}

#[test]
fn compile_exposes_automation_metadata_in_queue_manifest() {
    let workflow = automated_spec().validate().expect("automation validates");
    let compiled = workflow.compile();
    let queue = compiled
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "landing")
        .expect("queue manifest exists");

    assert_eq!(
        queue.automation,
        Some(QueueAutomation {
            actor: RoleId::new("mechanical"),
            transition: TransitionId::new("land_pr"),
            executor: None,
            outcomes: BTreeMap::from([(
                VerdictId::merge_conflict(),
                TransitionId::new("route_merge_conflict"),
            )]),
        })
    );
    assert!(queue.subscribers.is_empty());
}

#[test]
fn general_automation_outcome_validates_and_compiles() {
    let mut spec = automated_spec();
    let automation = automation_mut(&mut spec);
    automation.on_merge_conflict = None;
    automation
        .outcomes
        .insert("rejected".to_string(), "route_merge_conflict".to_string());

    let workflow = spec.validate().expect("general outcome validates");
    let queue = workflow
        .queues()
        .iter()
        .find(|queue| queue.id.as_str() == "landing")
        .expect("queue exists");
    let automation = queue.automation.as_ref().expect("automation present");
    assert_eq!(
        automation.outcome(&VerdictId::new("rejected")),
        Some(&TransitionId::new("route_merge_conflict"))
    );
    assert_eq!(automation.merge_conflict(), None);
}

#[test]
fn unauthorized_general_outcome_is_diagnosed() {
    let mut spec = automated_spec();
    let automation = automation_mut(&mut spec);
    automation.on_merge_conflict = None;
    automation
        .outcomes
        .insert("rejected".to_string(), "route_merge_conflict".to_string());
    // Strip the actor from the outcome transition (transitions[1]).
    spec.transitions[1].roles.clear();

    let errors = spec.validate().expect_err("unauthorized outcome fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationOutcomeUnauthorized {
                queue: "landing".to_string(),
                verdict: "rejected".to_string(),
                actor: "mechanical".to_string(),
                transition: "route_merge_conflict".to_string(),
            })
    );
}

#[test]
fn general_outcome_artifact_mismatch_is_diagnosed() {
    let mut spec = automated_spec();
    add_issue_artifact_kind(&mut spec);
    let automation = automation_mut(&mut spec);
    automation.on_merge_conflict = None;
    automation
        .outcomes
        .insert("rejected".to_string(), "route_merge_conflict".to_string());
    spec.transitions[1].artifact = "work_item".to_string();

    let errors = spec
        .validate()
        .expect_err("outcome artifact mismatch fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::QueueAutomationOutcomeArtifactMismatch {
                queue: "landing".to_string(),
                verdict: "rejected".to_string(),
                transition: "route_merge_conflict".to_string(),
                expected: "implementation_pr".to_string(),
                actual: "work_item".to_string(),
            })
    );
}

#[test]
fn transition_outcome_routing_validates() {
    let mut spec = automated_spec();
    // Route a verdict from the primary transition to the (same artifact/actor)
    // secondary transition.
    spec.transitions[0]
        .outcomes
        .insert("escalate".to_string(), "route_merge_conflict".to_string());

    let workflow = spec.validate().expect("transition outcome validates");
    let routed = workflow
        .transitions()
        .iter()
        .find(|transition| transition.id.as_str() == "land_pr")
        .expect("primary transition exists");
    assert_eq!(
        routed.outcomes.get(&VerdictId::new("escalate")),
        Some(&TransitionId::new("route_merge_conflict"))
    );
}

#[test]
fn unknown_transition_outcome_is_diagnosed() {
    let mut spec = automated_spec();
    spec.transitions[0]
        .outcomes
        .insert("escalate".to_string(), "missing_outcome".to_string());

    let errors = spec
        .validate()
        .expect_err("unknown transition outcome fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Transition,
                id: "missing_outcome".to_string(),
                site: ReferenceSite::TransitionOutcome {
                    transition: "land_pr".to_string(),
                    verdict: "escalate".to_string(),
                },
            })
    );
}

#[test]
fn transition_outcome_artifact_mismatch_is_diagnosed() {
    let mut spec = automated_spec();
    add_issue_artifact_kind(&mut spec);
    spec.transitions[0]
        .outcomes
        .insert("escalate".to_string(), "route_merge_conflict".to_string());
    // Make the outcome transition act on a different artifact than the primary.
    spec.transitions[1].artifact = "work_item".to_string();

    let errors = spec
        .validate()
        .expect_err("transition outcome artifact mismatch fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::TransitionOutcomeArtifactMismatch {
                transition: "land_pr".to_string(),
                verdict: "escalate".to_string(),
                outcome_transition: "route_merge_conflict".to_string(),
                expected: "implementation_pr".to_string(),
                actual: "work_item".to_string(),
            })
    );
}

#[test]
fn unauthorized_transition_outcome_is_diagnosed() {
    let mut spec = automated_spec();
    spec.transitions[0]
        .outcomes
        .insert("escalate".to_string(), "route_merge_conflict".to_string());
    // The primary transition authorizes `mechanical`; give the outcome a
    // disjoint role set so no role is shared.
    spec.transitions[1].roles = vec!["other".to_string()];
    spec.roles.push(role("other"));

    let errors = spec
        .validate()
        .expect_err("unauthorized transition outcome fails");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::TransitionOutcomeUnauthorized {
                transition: "land_pr".to_string(),
                verdict: "escalate".to_string(),
                outcome_transition: "route_merge_conflict".to_string(),
            })
    );
}
