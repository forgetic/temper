//! Hermetic observability smoke proof for a small reference-delivery movement.
//!
//! The test drives real workflow scan/execution against the in-memory Forge and
//! asserts that the runner's `temper-log` glue (`work_item_ref`, `labels_delta`)
//! feeds the new structured `emit_*` sites correctly: the §7 `transition.applied`
//! event carries the repo-qualified `artifact.ref` join key, the real label
//! delta as a typed field, and the generated §7 human line — all from one emit
//! site (the JSON-string-in-message path is gone).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{CreateIssue, CreateRepository, Forge, IssueQuery, RepositoryId};
use temper_log::WorkItemRef;
use temper_log::emit::{
    GateEvaluated, QueueEntered, TransitionApplied, emit_gate_evaluated, emit_queue_entered,
    emit_transition_applied,
};
use temper_runner::{
    WorkItemIdentity, gate_summary, labels_delta, queue_after_transition, scan_role, work_item_ref,
};
use temper_testing::{block_on, workflow};
use temper_workflow::{ArtifactSource, CiStatus, GateSignals, RoleId, TransitionId};
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

/// One captured event: its target plus all field name → rendered value pairs.
#[derive(Clone, Debug, Default)]
struct Captured {
    target: String,
    fields: BTreeMap<String, String>,
}

#[derive(Default)]
struct Visitor {
    fields: BTreeMap<String, String>,
}

impl Visit for Visitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(Captured {
            target: event.metadata().target().to_string(),
            fields: visitor.fields,
        });
    }
}

#[test]
fn applied_transition_emits_join_key_label_delta_and_human_line() {
    let forge = MemoryForge::new();
    let repo = create_repo(&forge);
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("architect");
    let now = DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is valid");

    let intake = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Coordinate greeting".to_string(),
            body: String::new(),
            labels: vec!["untriaged".to_string()],
            assignees: Vec::new(),
        },
    ))
    .expect("intake issue is created");

    let items = block_on(scan_role(&forge, &repo, &workflow, &compiled, now, &role))
        .expect("scan succeeds");
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(
        item.target,
        ArtifactSource::Issue {
            number: intake.number
        }
    );

    // The runner builds a stable identity for the selected item; the design's
    // join key is derived from it (provider scheme stripped, repo-qualified).
    let identity = WorkItemIdentity::new(&repo, &role, &item.queue, item.target, &item.kind)
        .with_tick_id("tick/smoke/1");
    // The memory forge's RepositoryId has no provider scheme, so the bare id is
    // used verbatim; the join key is repo-qualified and ends with the issue
    // number (#1, the first issue created in this repo).
    let join_key = work_item_ref(&identity);
    let join_key_string = join_key.to_string();
    assert!(
        join_key_string.ends_with("#1"),
        "join key is repo-qualified: {join_key_string}"
    );

    // Execute a real transition and emit `transition.applied` exactly as the
    // runner glue does: fields from the identity + applied effects.
    let transition = TransitionId::new("triage_to_code");
    let report = block_on(workflow.executor(&forge).execute(
        &repo,
        item.target,
        &transition,
        &role,
    ))
    .expect("transition executes");
    let delta = labels_delta(&report.applied);

    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);
    with_default(subscriber, || {
        emit_transition_applied(TransitionApplied {
            item: &join_key,
            transition: report.transition.as_str(),
            detail: "",
            labels_delta: &delta,
        });
    });

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1, "exactly one info event emitted");
    let ev = &captured[0];

    // Real structured fields on the engine plane — not a JSON string in message.
    assert_eq!(ev.target, "temper::engine");
    assert_eq!(ev.fields.get("service").map(String::as_str), Some("engine"));
    assert_eq!(
        ev.fields.get("event").map(String::as_str),
        Some("transition.applied")
    );
    // The join key equals the human tag's inner string (§3 rule 2).
    assert_eq!(
        ev.fields.get("artifact.ref").map(String::as_str),
        Some(join_key_string.as_str())
    );
    assert_eq!(
        ev.fields.get("transition").map(String::as_str),
        Some("triage_to_code")
    );
    // The label delta is the real, signed delta computed from applied effects.
    let labels = ev
        .fields
        .get("labels.delta")
        .cloned()
        .expect("labels.delta present");
    assert!(
        labels.contains("-untriaged"),
        "delta removes untriaged: {labels}"
    );
    assert!(labels.contains("+code"), "delta adds code: {labels}");
    assert!(labels.contains("+ready"), "delta adds ready: {labels}");

    // The human message is the §7 line, generated from the same inputs.
    let message = ev
        .fields
        .get("message")
        .cloned()
        .expect("human message present");
    assert!(
        message.starts_with(&format!(
            "engine:  [{join_key_string}] triage_to_code applied"
        )),
        "human line carries the padded service prefix, subject tag, and transition: {message}"
    );
    assert!(
        !message.contains("Coordinate greeting"),
        "no issue body leaks"
    );

    // Ground truth: the issue actually moved (the emit reflects a real change).
    let issues =
        block_on(forge.list_issues(&repo, IssueQuery::default())).expect("issues are readable");
    assert!(issues.iter().any(|issue| {
        issue.number == intake.number
            && issue.labels.iter().any(|label| label == "code")
            && issue.labels.iter().any(|label| label == "ready")
    }));
}

#[test]
fn applied_transition_derives_queue_entered_destination_and_role() {
    let workflow = workflow();
    let compiled = workflow.compile();

    // `triage_to_code` flips the issue into the `ready`-gated `code_ready`
    // queue, which the `engineer` role subscribes to — exactly §7's
    // `-> queue 'code_ready' | awaiting engineer`.
    let report = {
        let forge = MemoryForge::new();
        let repo = create_repo(&forge);
        let role = RoleId::new("architect");
        let intake = block_on(forge.create_issue(
            &repo,
            CreateIssue {
                title: "Queue move".to_string(),
                body: String::new(),
                labels: vec!["untriaged".to_string()],
                assignees: Vec::new(),
            },
        ))
        .expect("intake issue");
        block_on(workflow.executor(&forge).execute(
            &repo,
            ArtifactSource::Issue {
                number: intake.number,
            },
            &TransitionId::new("triage_to_code"),
            &role,
        ))
        .expect("transition executes")
    };

    let (queue, role) =
        queue_after_transition(&compiled, &report.applied).expect("a destination queue matches");
    assert_eq!(queue.id.as_str(), "code_ready");
    let note = role
        .map(|role| format!("awaiting {role}"))
        .unwrap_or_default();
    assert_eq!(note, "awaiting engineer");

    let item = WorkItemRef::issue("acme/service", 1);
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    with_default(registry().with(layer), || {
        emit_queue_entered(QueueEntered {
            item: &item,
            queue_to: queue.id.as_str(),
            note: &note,
        });
    });

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let ev = &captured[0];
    assert_eq!(ev.target, "temper::engine");
    assert_eq!(
        ev.fields.get("event").map(String::as_str),
        Some("queue.entered")
    );
    assert_eq!(
        ev.fields.get("queue.to").map(String::as_str),
        Some("code_ready")
    );
    let message = ev.fields.get("message").cloned().expect("message present");
    assert_eq!(
        message,
        "engine:  [acme/service#1] -> queue 'code_ready' | awaiting engineer"
    );
}

#[test]
fn gate_evaluated_renders_summary_and_note_from_signals() {
    let pr = WorkItemRef::pull_request("acme/service", 44);

    // Pending CI -> `waiting on CI`.
    let pending = GateSignals::new();
    let pending_summary = gate_summary(&pending);
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    with_default(registry().with(layer), || {
        emit_gate_evaluated(GateEvaluated {
            item: &pr,
            gates: &pending_summary,
            note: "waiting on CI",
        });
    });
    let captured = events.lock().unwrap();
    let ev = &captured[0];
    assert_eq!(ev.target, "temper::engine");
    assert_eq!(
        ev.fields.get("event").map(String::as_str),
        Some("gate.evaluated")
    );
    assert_eq!(
        ev.fields.get("gates").map(String::as_str),
        Some("ci_gate=pending dependency_gate=ok")
    );
    assert_eq!(
        ev.fields.get("message").cloned().unwrap(),
        "engine:  [acme/service PR#44] gates: ci_gate=pending dependency_gate=ok | waiting on CI"
    );
    drop(captured);

    // Passed CI -> eligible to land.
    let passed = GateSignals::new().with_ci(CiStatus::passed());
    assert_eq!(gate_summary(&passed), "ci_gate=ok dependency_gate=ok");
}

fn create_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "service".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .expect("repo is created")
    .id
}
