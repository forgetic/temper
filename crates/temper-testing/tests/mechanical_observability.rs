//! Structured tracing coverage for mechanical phases and real landing attempts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use chrono::Duration;
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_forge_model::{
    BranchRef, ChangeKind, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue,
    CreatePullRequest, CreateRepository, Forge, HintArtifactKind, RepositoryId, UserId,
};
use temper_runner::{MechanicalWorker, Worker};
use temper_testing::counting_forge::CountingForge;
use temper_testing::{block_on, ts};
use temper_workflow::{InMemoryJournal, LeasePolicy, RawWorkflowSpec, ValidatedWorkflow};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::with_default;
use tracing::{Instrument, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;
use tracing_subscriber::registry::LookupSpan;

const MECHANICAL_WORKFLOW: &str = r#"
{
  "name": "mechanical-observability",
  "roles": [{ "id": "mechanical" }],
  "labels": [
    { "id": "task" },
    { "id": "ready" },
    { "id": "done" },
    { "id": "implementation" },
    { "id": "landing" },
    { "id": "landed" }
  ],
  "artifact_kinds": [
    {
      "id": "task",
      "target": "issue",
      "identifying_labels": ["task"]
    },
    {
      "id": "implementation_pr",
      "target": "pull_request",
      "identifying_labels": ["implementation"]
    }
  ],
  "queues": [
    {
      "id": "ready_tasks",
      "artifact": "task",
      "labels": ["ready"],
      "automation": {
        "actor": "mechanical",
        "transition": "finish_task"
      }
    },
    {
      "id": "landing",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "automation": {
        "actor": "mechanical",
        "transition": "land_pr"
      }
    }
  ],
  "transitions": [
    {
      "id": "finish_task",
      "artifact": "task",
      "roles": ["mechanical"],
      "effects": [
        { "kind": "remove_label", "label": "ready" },
        { "kind": "add_label", "label": "done" }
      ]
    },
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "requires_gates": ["ci_gate"],
      "effects": [
        { "kind": "merge_pull_request" },
        { "kind": "remove_label", "label": "landing" },
        { "kind": "add_label", "label": "landed" }
      ]
    }
  ],
  "gates": [
    { "id": "ci_gate", "condition": { "kind": "ci_passed" } }
  ]
}
"#;

#[derive(Clone, Debug)]
enum FieldValue {
    Text(String),
    U64(u64),
    Bool(bool),
    Debug(String),
}

#[derive(Clone, Debug)]
struct Captured {
    target: String,
    level: Level,
    fields: BTreeMap<String, FieldValue>,
    span_fields: BTreeMap<String, FieldValue>,
}

impl Captured {
    fn text(&self, key: &str) -> Option<String> {
        field_text(self.fields.get(key))
    }

    fn span_text(&self, key: &str) -> Option<String> {
        field_text(self.span_fields.get(key))
    }

    fn u64(&self, key: &str) -> Option<u64> {
        match self.fields.get(key) {
            Some(FieldValue::U64(value)) => Some(*value),
            _ => None,
        }
    }

    fn bool(&self, key: &str) -> Option<bool> {
        match self.fields.get(key) {
            Some(FieldValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }
}

fn field_text(value: Option<&FieldValue>) -> Option<String> {
    match value {
        Some(FieldValue::Text(value)) => Some(value.clone()),
        Some(FieldValue::Debug(value)) => Some(value.trim_matches('"').to_string()),
        Some(FieldValue::U64(_)) | Some(FieldValue::Bool(_)) | None => None,
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, FieldValue>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(
            field.name().to_string(),
            FieldValue::Debug(format!("{value:?}")),
        );
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(
            field.name().to_string(),
            FieldValue::Text(value.to_string()),
        );
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), FieldValue::U64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), FieldValue::Bool(value));
    }
}

#[derive(Clone, Debug, Default)]
struct SpanFields(BTreeMap<String, FieldValue>);

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);
        ctx.span(id)
            .expect("new span is present in registry")
            .extensions_mut()
            .insert(SpanFields(visitor.fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut visitor = FieldVisitor::default();
        values.record(&mut visitor);
        let mut extensions = span.extensions_mut();
        let fields = extensions
            .get_mut::<SpanFields>()
            .expect("captured span fields were installed");
        fields.0.extend(visitor.fields);
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let mut span_fields = BTreeMap::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(fields) = span.extensions().get::<SpanFields>() {
                    span_fields.extend(fields.0.clone());
                }
            }
        }
        self.events.lock().expect("capture mutex").push(Captured {
            target: event.metadata().target().to_string(),
            level: *event.metadata().level(),
            fields: visitor.fields,
            span_fields,
        });
    }
}

fn capture(run: impl FnOnce()) -> Vec<Captured> {
    let layer = CaptureLayer::default();
    let events = Arc::clone(&layer.events);
    with_default(registry().with(layer), run);
    events.lock().expect("capture mutex").clone()
}

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(MECHANICAL_WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn gate_observation_workflow() -> ValidatedWorkflow {
    let mut raw: serde_json::Value =
        serde_json::from_str(MECHANICAL_WORKFLOW).expect("workflow parses");
    raw["queues"][1]["condition"] = serde_json::json!({"kind": "ci_passed"});
    let spec: RawWorkflowSpec = serde_json::from_value(raw).expect("workflow value parses");
    spec.validate().expect("workflow validates")
}

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

fn create_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "service".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "finish mechanically".to_string(),
            body: String::new(),
            labels: vec!["task".to_string(), "ready".to_string()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue is created");
}

fn create_landing_pr(
    forge: &MemoryForge,
    repo: &RepositoryId,
    suffix: &str,
) -> temper_forge_model::PullRequest {
    block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: format!("landing {suffix}"),
            body: String::new(),
            source: BranchRef {
                repository_id: repo.clone(),
                branch: format!("agent/{suffix}"),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".to_string(),
            },
            labels: vec!["implementation".to_string(), "landing".to_string()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request is created")
}

fn successful_ci(repo: &RepositoryId, pull_request: &temper_forge_model::PullRequest) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("ci-{}", pull_request.number.get())),
        repo_id: repo.clone(),
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: format!("head-{}", pull_request.number.get()),
        name: "required".to_string(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::Success),
        url: None,
        created_at: ts("2026-07-13T12:00:00Z"),
        started_at: Some(ts("2026-07-13T12:00:01Z")),
        completed_at: Some(ts("2026-07-13T12:00:02Z")),
        updated_at: ts("2026-07-13T12:00:02Z"),
    }
}

fn events_with_measurement<'a>(events: &'a [Captured], measurement: &str) -> Vec<&'a Captured> {
    events
        .iter()
        .filter(|event| event.text("measurement").as_deref() == Some(measurement))
        .collect()
}

fn assert_phase_common(event: &Captured, repository: &str, scope: &str, wake_run_id: &str) {
    assert_eq!(event.target, "temper::worker");
    assert_eq!(event.level, Level::DEBUG);
    assert_eq!(event.text("repo").as_deref(), Some(repository));
    assert_eq!(event.text("mechanical.scope").as_deref(), Some(scope));
    assert!(
        event.u64("duration_ms").is_some(),
        "numeric duration: {event:?}"
    );
    assert_eq!(
        event.span_text("wake.run_id").as_deref(),
        Some(wake_run_id),
        "phase inherits the admitted wake span"
    );
}

#[test]
fn broad_phase_measurements_include_provider_deltas_and_non_merge_has_no_attempt() {
    let memory = MemoryForge::new();
    let repo = create_repo(&memory);
    let repo_label = temper_log::strip_provider_scheme(repo.as_str()).to_string();
    create_ready_issue(&memory, &repo);
    let forge = CountingForge::new(memory);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &forge, &repo, &journal, lease_policy());

    let events = capture(|| {
        let wake = tracing::debug_span!("wake", wake.run_id = "acme/service:41");
        block_on(worker.tick(ts("2026-07-13T12:00:00Z")).instrument(wake))
            .expect("broad mechanical tick succeeds");
    });

    let phases = events_with_measurement(&events, "mechanical.phase");
    assert_eq!(phases.len(), 3, "one terminal event per broad phase");
    let names = phases
        .iter()
        .map(|event| event.text("mechanical.phase").expect("phase name"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from([
            "reconciliation".to_string(),
            "automated_scan".to_string(),
            "transition_application".to_string(),
        ])
    );
    for phase in phases {
        assert_phase_common(phase, &repo_label, "broad", "acme/service:41");
        assert_eq!(phase.text("outcome").as_deref(), Some("success"));
        assert_eq!(phase.bool("provider.requests_available"), Some(true));
        assert!(
            phase.u64("provider.request_total").is_some(),
            "provider request delta remains numeric: {phase:?}"
        );
    }
    assert!(
        events_with_measurement(&events, "mechanical.landing_attempt").is_empty(),
        "a direct non-merge automation is not a landing attempt"
    );
}

#[test]
fn targeted_phases_and_repeated_gate_observations_keep_wake_correlation() {
    let memory = MemoryForge::new();
    let repo = create_repo(&memory);
    let repo_label = temper_log::strip_provider_scheme(repo.as_str()).to_string();
    let pull_request = create_landing_pr(&memory, &repo, "pending");
    let forge = CountingForge::new(memory);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &forge, &repo, &journal, lease_policy());

    let events = capture(|| {
        for generation in [42_u64, 43_u64] {
            let run_id = format!("acme/service:{generation}");
            let wake = tracing::debug_span!("wake", wake.run_id = run_id.as_str());
            block_on(
                worker
                    .tick_artifact(
                        ts("2026-07-13T12:00:00Z"),
                        pull_request.number,
                        HintArtifactKind::PullRequest,
                        ChangeKind::Ci,
                    )
                    .instrument(wake),
            )
            .expect("gate miss is a successful targeted pass");
        }
    });

    let phases = events_with_measurement(&events, "mechanical.phase");
    assert_eq!(phases.len(), 6, "both targeted passes finish all phases");
    let phase_names = phases
        .iter()
        .map(|event| event.text("mechanical.phase").expect("phase name"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        phase_names,
        BTreeSet::from([
            "reconciliation".to_string(),
            "automated_scan".to_string(),
            "transition_application".to_string(),
        ])
    );
    for phase in phases {
        assert_eq!(phase.text("mechanical.scope").as_deref(), Some("targeted"));
        assert_eq!(phase.text("outcome").as_deref(), Some("success"));
        assert_eq!(phase.bool("provider.requests_available"), Some(true));
        assert!(phase.u64("provider.request_total").is_some());
        assert!(phase.u64("duration_ms").is_some());
        assert!(matches!(
            phase.span_text("wake.run_id").as_deref(),
            Some("acme/service:42" | "acme/service:43")
        ));
        let artifact_ref = format!("{repo_label} PR#{}", pull_request.number.get());
        assert_eq!(
            phase.text("artifact.ref").as_deref(),
            Some(artifact_ref.as_str())
        );
    }

    let attempts = events_with_measurement(&events, "mechanical.landing_attempt");
    assert_eq!(attempts.len(), 4, "each execution attempt has two events");
    for pair in attempts.chunks_exact(2) {
        assert_eq!(pair[0].text("landing.outcome").as_deref(), Some("started"));
        assert_eq!(
            pair[1].text("landing.outcome").as_deref(),
            Some("gate_not_satisfied")
        );
        assert!(pair[1].u64("duration_ms").is_some());
    }

    // Queue-level CI matching performs a repeatable read even when no direct
    // transition attempt is eligible. Scan the same pending PR twice and prove
    // those observations retain their fields while staying below info.
    let gate_workflow = gate_observation_workflow();
    let gate_journal = InMemoryJournal::new();
    let gate_worker =
        MechanicalWorker::new(&gate_workflow, &forge, &repo, &gate_journal, lease_policy());
    let gate_events = capture(|| {
        for generation in [46_u64, 47_u64] {
            let run_id = format!("acme/service:{generation}");
            let wake = tracing::debug_span!("wake", wake.run_id = run_id.as_str());
            block_on(
                gate_worker
                    .tick_artifact(
                        ts("2026-07-13T12:00:00Z"),
                        pull_request.number,
                        HintArtifactKind::PullRequest,
                        ChangeKind::Ci,
                    )
                    .instrument(wake),
            )
            .expect("pending gate observation succeeds");
        }
    });
    let gate_events = gate_events
        .iter()
        .filter(|event| event.text("event").as_deref() == Some("gate.evaluated"))
        .collect::<Vec<_>>();
    assert_eq!(
        gate_events.len(),
        2,
        "the same read-side gate is observed twice"
    );
    assert!(
        gate_events.iter().all(|event| event.level == Level::DEBUG),
        "repeatable gate observations never return to info"
    );
}

#[test]
fn landing_attempt_pairs_started_with_applied_terminal_outcome() {
    let memory = MemoryForge::new();
    let repo = create_repo(&memory);
    let repo_label = temper_log::strip_provider_scheme(repo.as_str()).to_string();
    let pull_request = create_landing_pr(&memory, &repo, "green");
    memory.seed_ci_jobs(&repo, vec![successful_ci(&repo, &pull_request)]);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &memory, &repo, &journal, lease_policy());

    let events = capture(|| {
        let wake = tracing::debug_span!("wake", wake.run_id = "acme/service:44");
        block_on(
            worker
                .tick_artifact(
                    ts("2026-07-13T12:00:03Z"),
                    pull_request.number,
                    HintArtifactKind::PullRequest,
                    ChangeKind::Ci,
                )
                .instrument(wake),
        )
        .expect("green pull request lands");
    });

    let attempts = events_with_measurement(&events, "mechanical.landing_attempt");
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0].text("landing.outcome").as_deref(),
        Some("started")
    );
    assert_eq!(
        attempts[1].text("landing.outcome").as_deref(),
        Some("applied")
    );
    assert!(attempts[1].u64("duration_ms").is_some());
    for event in attempts {
        assert_eq!(event.level, Level::DEBUG);
        assert_eq!(event.text("repo").as_deref(), Some(repo_label.as_str()));
        assert_eq!(event.u64("pr.number"), Some(pull_request.number.get()));
        assert_eq!(event.text("queue").as_deref(), Some("landing"));
        assert_eq!(event.text("transition").as_deref(), Some("land_pr"));
        assert_eq!(
            event.span_text("wake.run_id").as_deref(),
            Some("acme/service:44")
        );
    }
}

#[test]
fn failed_targeted_scan_emits_terminal_duration_provider_delta_and_wake_id() {
    let memory = MemoryForge::new();
    let repo = create_repo(&memory);
    let repo_label = temper_log::strip_provider_scheme(repo.as_str()).to_string();
    let pull_request = create_landing_pr(&memory, &repo, "failure");
    memory.fail_next(FaultOp::GetPullRequestByNumber, "synthetic read failure");
    let forge = CountingForge::new(memory);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(&workflow, &forge, &repo, &journal, lease_policy());

    let events = capture(|| {
        let wake = tracing::debug_span!("wake", wake.run_id = "acme/service:45");
        let result = block_on(
            worker
                .tick_artifact(
                    ts("2026-07-13T12:00:04Z"),
                    pull_request.number,
                    HintArtifactKind::PullRequest,
                    ChangeKind::Ci,
                )
                .instrument(wake),
        );
        assert!(
            result.is_err(),
            "the injected Forge failure reaches the caller"
        );
    });

    let phases = events_with_measurement(&events, "mechanical.phase");
    assert_eq!(
        phases.len(),
        1,
        "only the phase that started is terminally measured"
    );
    let failed = phases[0];
    assert_phase_common(failed, &repo_label, "targeted", "acme/service:45");
    assert_eq!(
        failed.text("mechanical.phase").as_deref(),
        Some("automated_scan")
    );
    assert_eq!(failed.text("outcome").as_deref(), Some("failed"));
    assert_eq!(failed.bool("provider.requests_available"), Some(true));
    assert_eq!(failed.u64("provider.request_total"), Some(1));
}
