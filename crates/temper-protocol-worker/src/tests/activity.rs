// SPDX-License-Identifier: MPL-2.0

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityAcknowledgement, AgentActivityBatch,
    AgentActivityCapturePolicyV1,
};

use crate::{
    WORKER_PROTOCOL_VERSION, WorkerActivityAcknowledgement, WorkerActivityBatch,
    WorkerProtocolMessage,
};

#[test]
fn activity_batch_and_ack_round_trip_without_changing_shared_dtos() {
    let batch = WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-1".to_string(),
        assignment_id: "assignment-1".to_string(),
        capture_policy: AgentActivityCapturePolicyV1::default(),
        batch: AgentActivityBatch {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: "run-1".to_string(),
            first_seq: 1,
            events: Vec::new(),
            blobs: Vec::new(),
        },
    });
    let encoded = serde_json::to_vec(&batch).expect("serialize batch");
    let decoded: WorkerProtocolMessage = serde_json::from_slice(&encoded).expect("parse batch");
    assert_eq!(decoded, batch);

    let ack = WorkerProtocolMessage::ActivityAck(WorkerActivityAcknowledgement {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-1".to_string(),
        acknowledgement: AgentActivityAcknowledgement {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: "run-1".to_string(),
            highest_contiguous_seq: 3,
        },
    });
    let encoded = serde_json::to_vec(&ack).expect("serialize ack");
    let decoded: WorkerProtocolMessage = serde_json::from_slice(&encoded).expect("parse ack");
    assert_eq!(decoded, ack);
}
