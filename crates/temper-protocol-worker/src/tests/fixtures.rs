// SPDX-License-Identifier: MPL-2.0

use crate::{WORKER_PROTOCOL_VERSION, WorkerProtocolMessage};

fn assert_round_trips(json: &str) -> WorkerProtocolMessage {
    let msg: WorkerProtocolMessage = serde_json::from_str(json).expect("fixture parses");
    let encoded = serde_json::to_string(&msg).expect("serializes");
    let again: WorkerProtocolMessage = serde_json::from_str(&encoded).expect("round-trips");
    assert_eq!(msg, again, "round-trip must be lossless");
    msg
}

fn protocol_version(msg: &WorkerProtocolMessage) -> u32 {
    match msg {
        WorkerProtocolMessage::Register(msg) => msg.protocol_version,
        WorkerProtocolMessage::Poll(msg) => msg.protocol_version,
        WorkerProtocolMessage::Assign(msg) => msg.protocol_version,
        WorkerProtocolMessage::Heartbeat(msg) => msg.protocol_version,
        WorkerProtocolMessage::Result(msg) => msg.protocol_version,
        WorkerProtocolMessage::Release(msg) => msg.protocol_version,
        WorkerProtocolMessage::LeaseAck(msg) => msg.protocol_version,
        WorkerProtocolMessage::FetchContext(msg) => msg.protocol_version,
        WorkerProtocolMessage::ContextResponse(msg) => msg.protocol_version,
        WorkerProtocolMessage::Error(msg) => msg.protocol_version,
    }
}

fn fixture_jsons() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/reference/worker-daemon-wire-protocol/examples");
    let mut fixtures = std::fs::read_dir(dir)
        .expect("read protocol fixture directory")
        .map(|entry| entry.expect("fixture entry"))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("json")
        })
        .map(|entry| {
            let path = entry.path();
            let name = path
                .file_name()
                .expect("fixture has a filename")
                .to_str()
                .expect("fixture filename is UTF-8")
                .to_string();
            let json = std::fs::read_to_string(path).expect("fixture is readable");
            (name, json)
        })
        .collect::<Vec<_>>();
    fixtures.sort_by(|left, right| left.0.cmp(&right.0));
    fixtures
}

#[test]
fn fixtures_round_trip_and_match_variants() {
    for (filename, json) in fixture_jsons() {
        let msg = assert_round_trips(&json);
        assert_eq!(protocol_version(&msg), WORKER_PROTOCOL_VERSION);

        let expected = filename.trim_end_matches(".json");
        match (expected, msg) {
            ("register", WorkerProtocolMessage::Register(_))
            | ("poll", WorkerProtocolMessage::Poll(_))
            | ("assign", WorkerProtocolMessage::Assign(_))
            | ("heartbeat", WorkerProtocolMessage::Heartbeat(_))
            | ("result", WorkerProtocolMessage::Result(_))
            | ("result-verdict", WorkerProtocolMessage::Result(_))
            | ("result-verdict-children", WorkerProtocolMessage::Result(_))
            | ("release", WorkerProtocolMessage::Release(_))
            | ("lease-ack", WorkerProtocolMessage::LeaseAck(_))
            | ("fetch-context", WorkerProtocolMessage::FetchContext(_))
            | ("context-response", WorkerProtocolMessage::ContextResponse(_))
            | ("context-response-error", WorkerProtocolMessage::ContextResponse(_))
            | ("error", WorkerProtocolMessage::Error(_)) => {}
            (name, msg) => panic!("{name} parsed as unexpected variant: {msg:?}"),
        }
    }
}

#[test]
fn unknown_fields_are_ignored() {
    let msg: WorkerProtocolMessage = serde_json::from_str(
        r#"{"type":"poll","protocol_version":1,"worker_id":"w1","free_capacity":2,"future_field":"ignored"}"#,
    )
    .expect("unknown fields must be accepted");

    assert!(matches!(msg, WorkerProtocolMessage::Poll(_)));
}
