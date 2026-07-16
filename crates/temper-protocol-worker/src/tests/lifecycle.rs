// SPDX-License-Identifier: MPL-2.0

use crate::{
    Heartbeat, JobCancellationState, JobHeartbeatPhase, JobOperationKind, JobResultDeliveryState,
    JobResultDurabilityState, JobTimeoutReason,
};

#[test]
fn structured_liveness_round_trips_without_sensitive_content() {
    let json = r#"{
      "protocol_version":1,
      "worker_id":"worker-1",
      "jobs":[{
        "job_id":"job-1",
        "attempt_id":"attempt-1",
        "state":"finishing",
        "message":"cancelling",
        "liveness":{
          "phase":"cancel_requested",
          "run_elapsed_ms":91000,
          "no_progress_elapsed_ms":31000,
          "active_operation_count":12,
          "active_operations":[{
            "scope":"main",
            "kind":"tool",
            "name":"forge_list_related",
            "operation_id":"call-7",
            "elapsed_ms":31000
          }],
          "timeout":{"reason":"no_progress","limit_ms":30000},
          "cancellation":"requested",
          "result_durability":"pending",
          "result_delivery":"not_ready",
          "pending_result":true
        }
      }]
    }"#;
    let heartbeat: Heartbeat = serde_json::from_str(json).expect("structured heartbeat");
    let liveness = heartbeat.jobs[0].liveness.as_ref().unwrap();
    assert_eq!(liveness.phase, JobHeartbeatPhase::CancelRequested);
    assert_eq!(liveness.active_operation_count, 12);
    assert_eq!(liveness.active_operations[0].kind, JobOperationKind::Tool);
    assert_eq!(
        liveness.timeout.as_ref().unwrap().reason,
        JobTimeoutReason::NoProgress
    );
    assert_eq!(liveness.cancellation, JobCancellationState::Requested);
    assert_eq!(
        liveness.result_durability,
        JobResultDurabilityState::Pending
    );
    assert_eq!(liveness.result_delivery, JobResultDeliveryState::NotReady);

    let encoded = serde_json::to_string(&heartbeat).unwrap();
    for forbidden in ["arguments", "result_body", "prompt", "credential", "secret"] {
        assert!(!encoded.contains(forbidden), "heartbeat leaked {forbidden}");
    }
}

#[test]
fn legacy_heartbeat_without_liveness_remains_compatible() {
    let heartbeat: Heartbeat = serde_json::from_str(
        r#"{"protocol_version":1,"worker_id":"legacy","jobs":[{"job_id":"job","state":"running","message":"coding"}]}"#,
    )
    .expect("legacy heartbeat");
    assert_eq!(heartbeat.jobs[0].attempt_id, None);
    assert_eq!(heartbeat.jobs[0].liveness, None);
    let encoded = serde_json::to_value(heartbeat).unwrap();
    assert!(encoded["jobs"][0].get("liveness").is_none());
}
