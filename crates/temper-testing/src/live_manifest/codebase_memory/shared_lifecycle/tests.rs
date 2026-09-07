use std::path::PathBuf;

use serde_json::{Value, json};

use super::processes::{Cohort, Identity, cancellation_complete, validate_continuity};

fn cleanup_events(attempt: &str) -> Vec<Value> {
    [
        json!({"event":"worker.job.cancellation_requested","timeout_reason":"ownership_lost"}),
        json!({"event":"worker.containment.cleanup_completed","owner_kind":"mcp_server","root_pid":40,"direct_child_reap":"reaped","recursive_empty":"proven"}),
        json!({"event":"worker.job.cancellation_completed"}),
    ].into_iter().map(|mut fields| {
        fields["job_id"] = json!("a-job");
        fields["attempt_id"] = json!(attempt);
        json!({"fields":fields})
    }).collect()
}

#[test]
fn cancellation_requires_exact_attempt_serving_owner_and_joined_cleanup() {
    let frontend = Identity { pid: 4, start: 4 };
    let owner = Identity { pid: 40, start: 40 };
    let mut events = cleanup_events("a-attempt");
    assert!(cancellation_complete(
        &events,
        "a-job",
        "a-attempt",
        &frontend,
        &owner
    ));
    assert!(!cancellation_complete(
        &events,
        "a-job",
        "replacement",
        &frontend,
        &owner
    ));
    events[1]["fields"]["owner_kind"] = json!("worker_command");
    assert!(!cancellation_complete(
        &events,
        "a-job",
        "a-attempt",
        &frontend,
        &owner
    ));
    events[1]["fields"]["owner_kind"] = json!("mcp_server");
    events[1]["fields"]["root_pid"] = json!(99);
    assert!(!cancellation_complete(
        &events,
        "a-job",
        "a-attempt",
        &frontend,
        &owner
    ));
    events[1]["fields"]["root_pid"] = json!(40);
    events.swap(0, 2);
    assert!(!cancellation_complete(
        &events,
        "a-job",
        "a-attempt",
        &frontend,
        &owner
    ));
    events.swap(0, 2);
    events[1]["fields"]["recursive_empty"] = json!("not_proven");
    assert!(!cancellation_complete(
        &events,
        "a-job",
        "a-attempt",
        &frontend,
        &owner
    ));
}

#[test]
fn continuity_rejects_zero_session_handoff_and_daemon_restart() {
    let initial = vec![
        json!({"event":"daemon_started","identity":{"pid":1,"start":1}}),
        json!({"event":"work_started","identity":{"pid":2,"start":2}}),
    ];
    let mut rows = initial.clone();
    rows.extend([
        json!({"event":"attached","active":1}),
        json!({"event":"attached","active":2}),
        json!({"event":"detached","active":1}),
    ]);
    assert!(validate_continuity(&rows).is_ok());
    rows.push(json!({"event":"detached","active":0}));
    assert!(validate_continuity(&rows).is_err());
    rows.push(json!({"event":"last_session_closed"}));
    assert!(validate_continuity(&rows).is_ok());
    rows.push(initial[0].clone());
    assert!(validate_continuity(&rows).is_err());
}

#[test]
fn survivor_sentinel_rejects_pre_cleanup_wrong_process_and_wrong_bytes() {
    let cohort = Cohort {
        daemon: Identity { pid: 1, start: 1 },
        work: Identity { pid: 2, start: 2 },
        owner: Identity { pid: 3, start: 3 },
        a_frontend: Identity { pid: 4, start: 4 },
        a_process_owner: Identity { pid: 40, start: 40 },
        a_cwd: "/a".into(),
    };
    let b = Identity { pid: 5, start: 5 };
    let mut rows = ["search_graph","get_code_snippet","trace_path","get_code_snippet","get_code_snippet"].into_iter().enumerate().map(|(index,tool)| json!({"event":"graph_result","at":index+101,"session":"5","frontend":{"pid":5,"start":5},"tool":tool,"root":"/b/demo","worker":{"pid":2,"start":2}})).collect::<Vec<_>>();
    rows[1]["selector"] = json!("fixture::retry_worker_topic");
    rows[1]["source"] = json!(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/shared-codebase-memory-lifecycle/repo/src/lib.rs"
    )));
    assert!(super::evidence::survivor_source(&rows, &cohort, &b, "/b/demo", 100).is_ok());
    rows[0]["root"] = json!("/");
    assert!(super::evidence::survivor_source(&rows, &cohort, &b, "/b/demo", 100).is_err());
    rows[0]["root"] = json!("/b/demo");
    assert!(super::evidence::survivor_source(&rows, &cohort, &b, "/b/demo", 103).is_err());
    rows[1]["worker"]["start"] = json!(9);
    assert!(super::evidence::survivor_source(&rows, &cohort, &b, "/b/demo", 100).is_err());
    rows[1]["worker"]["start"] = json!(2);
    rows[1]["source"] = json!("another checkout source");
    assert!(super::evidence::survivor_source(&rows, &cohort, &b, "/b/demo", 100).is_err());
}

#[test]
fn lifecycle_bundle_declares_two_real_jobs_and_required_correlated_assertions() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scenarios/shared-codebase-memory-lifecycle");
    let bundle = crate::live_manifest::ScenarioBundle::load(&path).expect("lifecycle bundle");
    assert!(
        bundle
            .issue("owner")
            .unwrap()
            .labels
            .contains(&"ready".into())
    );
    assert!(
        !bundle
            .issue("source")
            .unwrap()
            .labels
            .contains(&"ready".into())
    );
    assert_eq!(bundle.poll_cadence.as_secs(), 1);
    assert_eq!(bundle.ci_poll_cadence.as_secs(), 1);
    let manifest: toml::Value = std::fs::read_to_string(path.join("scenario.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        manifest["validation"]["feature"].as_str(),
        Some("ai/temper#1280")
    );
    assert_eq!(manifest["assertions"][0]["required"].as_bool(), Some(true));
    assert_eq!(
        manifest["expect"]["sequence"][0]["required"].as_bool(),
        Some(true)
    );
}
