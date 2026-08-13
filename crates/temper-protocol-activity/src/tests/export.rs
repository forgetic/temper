use serde_json::json;

use super::*;

#[test]
fn trace_export_records_round_trip_events_and_attachments() {
    let event = usage_event(7);
    let event_record = TraceExportRecordV1::event(event.clone());
    assert_eq!(event_record.version(), TRACE_EXPORT_RECORD_VERSION);
    assert_eq!(
        serde_json::to_value(&event_record).unwrap(),
        json!({
            "type": "agent_run_event_v1",
            "version": TRACE_EXPORT_RECORD_VERSION,
            "event": event,
        })
    );
    let rendered_event = serde_json::to_string(&event_record).unwrap();
    assert!(
        rendered_event.starts_with("{\"type\":\"agent_run_event_v1\",\"version\":1,\"event\":")
    );
    assert_eq!(
        serde_json::from_str::<TraceExportRecordV1>(&rendered_event).unwrap(),
        event_record
    );

    let attachment =
        BlobAttachmentV1::from_bytes(BlobMediaTypeV1::TextPlainUtf8, b"exported transcript");
    let attachment_record = TraceExportRecordV1::attachment(attachment.clone());
    assert_eq!(
        serde_json::to_value(&attachment_record).unwrap(),
        json!({
            "type": "blob_attachment_v1",
            "version": TRACE_EXPORT_RECORD_VERSION,
            "attachment": attachment,
        })
    );
    let rendered_attachment = serde_json::to_string(&attachment_record).unwrap();
    assert!(
        rendered_attachment
            .starts_with("{\"type\":\"blob_attachment_v1\",\"version\":1,\"attachment\":")
    );
    assert_eq!(
        serde_json::from_str::<TraceExportRecordV1>(&rendered_attachment).unwrap(),
        attachment_record
    );

    let transcript_record =
        TraceExportRecordV1::operator_transcript(OperatorTranscriptToolResultV1 {
            version: OPERATOR_TRANSCRIPT_RECORD_VERSION,
            call_id: "graph-read".to_string(),
            tool_name: "codebase_memory_search_graph".to_string(),
            model_result_text: InlineContentV1 {
                text: "cold stable upsert is ready".to_string(),
                truncated: false,
            },
        });
    let rendered_transcript = serde_json::to_string(&transcript_record).unwrap();
    assert!(rendered_transcript.contains("cold stable upsert is ready"));
    assert!(!format!("{transcript_record:?}").contains("cold stable upsert is ready"));
    assert_eq!(
        serde_json::from_str::<TraceExportRecordV1>(&rendered_transcript).unwrap(),
        transcript_record
    );
}

#[test]
fn trace_export_records_reject_unknown_fields() {
    let records = [
        serde_json::to_value(TraceExportRecordV1::event(usage_event(1))).unwrap(),
        serde_json::to_value(TraceExportRecordV1::attachment(
            BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, b"{}"),
        ))
        .unwrap(),
    ];

    for mut record in records {
        record["extension"] = json!("not part of v1");
        let error = serde_json::from_value::<TraceExportRecordV1>(record)
            .expect_err("unknown export fields must be rejected");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}

#[test]
fn trace_export_records_reject_unsupported_versions() {
    let records = [
        serde_json::to_value(TraceExportRecordV1::event(usage_event(1))).unwrap(),
        serde_json::to_value(TraceExportRecordV1::attachment(
            BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, b"{}"),
        ))
        .unwrap(),
    ];

    for mut record in records {
        record["version"] = json!(TRACE_EXPORT_RECORD_VERSION + 1);
        let error = serde_json::from_value::<TraceExportRecordV1>(record)
            .expect_err("future export versions must be rejected");
        assert!(
            error
                .to_string()
                .contains("unsupported trace export record version 2; expected 1"),
            "{error}"
        );
    }
}
