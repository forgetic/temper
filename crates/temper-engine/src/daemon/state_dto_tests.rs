// SPDX-License-Identifier: MPL-2.0

//! Wire-shape contract tests for the `GET /v1/state` snapshot DTOs.
//!
//! These pin the serialized JSON field set so the daemon backend and the
//! temper-web UI cannot drift. The raw snapshot's fields must be sufficient for
//! the board to project its cold-start cards
//! (`crates/temper-web/ui/fixtures/state-snapshot.json`): each job carries the
//! `ref` join key + `role` the board card model needs, plus `workers`.

use serde_json::{Value, json};
use temper_protocol_worker::{Artifact, Capability, Capacity, Register, WORKER_PROTOCOL_VERSION};
use temper_worker_registry::DaemonCore;

use super::{ArtifactDto, DaemonStateSnapshot, JobDto};

fn register(worker_id: &str, role: &str, repo: &str) -> Register {
    Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    }
}

fn issue(number: u64) -> Artifact {
    Artifact {
        item: json!(number),
        kind: "issue".to_string(),
    }
}

fn pull_request(number: u64) -> Artifact {
    Artifact {
        item: json!(number),
        kind: "pull_request".to_string(),
    }
}

/// Two workers (one in-flight, one idle), one in-flight job, one queued job
/// behind the busy role, exercising every snapshot section.
fn representative_core() -> DaemonCore {
    let mut core = DaemonCore::new();
    core.coordinator_mut()
        .register(&register("w-code-1", "code", "acme/widgets"));
    core.coordinator_mut()
        .register(&register("w-review-1", "review", "acme/api"));

    // Enqueue then dispatch one code job -> in-flight.
    core.enqueue_job("job-39", "code", "acme/widgets", issue(39), json!({"k": 1}));
    let assignment = core
        .coordinator_mut()
        .dispatch_next()
        .expect("the registered code worker takes the code job");
    assert_eq!(assignment.job_id, "job-39");

    // A second code job stays queued behind the now-busy code worker.
    core.enqueue_job("job-42", "code", "acme/widgets", issue(42), json!({"k": 2}));
    // A review PR job is queued (no review job dispatched, so role not saturated).
    core.enqueue_job("job-8", "review", "acme/api", pull_request(8), json!({}));
    core
}

#[test]
fn snapshot_has_the_expected_wire_shape() {
    let core = representative_core();
    let snapshot = DaemonStateSnapshot::from_core(&core);
    let value = snapshot.to_json();

    // Top-level field set the board consumes.
    let object = value.as_object().expect("snapshot is a JSON object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["in_flight", "queued", "role_saturation", "workers"]
    );

    // workers tile: matches the fixture's {healthy, total} shape exactly.
    assert_eq!(value["workers"], json!({ "healthy": 2, "total": 2 }));

    // in-flight job carries the board card's join key + role.
    let in_flight = value["in_flight"].as_array().expect("in_flight array");
    assert_eq!(in_flight.len(), 1);
    assert_eq!(
        in_flight[0],
        json!({
            "job_id": "job-39",
            "role": "code",
            "repo": "acme/widgets",
            "artifact": { "item": 39, "kind": "issue" },
            "ref": "acme/widgets#39",
        })
    );

    // queued jobs preserve enqueue order and render the right ref shapes.
    let queued = value["queued"].as_array().expect("queued array");
    let queued_refs: Vec<&Value> = queued.iter().map(|job| &job["ref"]).collect();
    assert_eq!(
        queued_refs,
        vec![&json!("acme/widgets#42"), &json!("acme/api PR#8")]
    );

    // role_saturation surfaces the code role's wait list (job-42 behind job-39).
    assert_eq!(
        value["role_saturation"],
        json!([
            { "role": "code", "concurrency": 1, "waiting": ["acme/widgets#42"] }
        ])
    );
}

#[test]
fn snapshot_field_names_match_board_card_projection_keys() {
    // The board's card model (state-snapshot.json) needs id, ref, role per card
    // and {healthy,total} workers. Assert the raw snapshot exposes those names
    // so PR D can project without renaming.
    let core = representative_core();
    let value = DaemonStateSnapshot::from_core(&core).to_json();

    let card_source = &value["in_flight"][0];
    assert!(card_source.get("job_id").is_some(), "card id source");
    assert!(card_source.get("ref").is_some(), "card ref join key");
    assert!(card_source.get("role").is_some(), "card role");

    let workers = &value["workers"];
    assert!(workers.get("healthy").is_some());
    assert!(workers.get("total").is_some());
}

#[test]
fn job_dto_serializes_one_in_flight_job_for_the_job_route() {
    let core = representative_core();
    let job = core
        .in_flight_job("job-39")
        .expect("job-39 is in flight after dispatch");
    let value = JobDto::from(&job).to_json();
    assert_eq!(
        value,
        json!({
            "job_id": "job-39",
            "role": "code",
            "repo": "acme/widgets",
            "artifact": { "item": 39, "kind": "issue" },
            "ref": "acme/widgets#39",
        })
    );
}

#[test]
fn empty_core_yields_an_empty_but_well_formed_snapshot() {
    let value = DaemonStateSnapshot::from_core(&DaemonCore::new()).to_json();
    assert_eq!(
        value,
        json!({
            "workers": { "healthy": 0, "total": 0 },
            "queued": [],
            "in_flight": [],
            "role_saturation": [],
        })
    );
}

#[test]
fn non_numeric_or_unknown_artifact_yields_null_ref() {
    let weird = Artifact {
        item: json!("not-a-number"),
        kind: "issue".to_string(),
    };
    let dto = ArtifactDto::from(&weird);
    assert_eq!(dto.kind, "issue");

    let mut core = DaemonCore::new();
    core.enqueue_job("job-x", "code", "acme/widgets", weird, json!({}));
    let value = DaemonStateSnapshot::from_core(&core).to_json();
    assert_eq!(value["queued"][0]["ref"], Value::Null);
}
