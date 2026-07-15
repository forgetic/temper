use serde_json::json;

use super::*;

fn prompt_snapshot() -> PromptSnapshotV1 {
    PromptSnapshotV1 {
        system_prompt: Some("You are exact.".into()),
        initial_user_message: "Inspect café.".into(),
        tools: vec![PromptToolDefinitionV1 {
            name: "read".into(),
            description: "Read a file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": false
            }),
        }],
    }
}

fn prompt_event() -> AgentRunEventV1 {
    let snapshot = prompt_snapshot();
    let canonical = snapshot.to_canonical_json_bytes().unwrap();
    let tools = snapshot.tools_to_canonical_json_bytes().unwrap();
    let mut event = usage_event(1);
    event.turn = Some(0);
    event.event = AgentActivityEventV1::PromptPrepared(PromptPreparedV1 {
        system_prompt_present: true,
        system_prompt_bytes: 14,
        initial_user_message_bytes: 14,
        tool_manifest_bytes: tools.len() as u64,
        tool_count: 1,
        original_snapshot_bytes: canonical.len() as u64,
        captured_bytes: canonical.len() as u64,
        disposition: PromptCaptureDispositionV1::Captured,
        content: Some(CapturedContentV1::Inline(InlineContentV1 {
            text: String::from_utf8(canonical).unwrap(),
            truncated: false,
        })),
    });
    event
}

fn prompt_child_record() -> AgentActivityChildRecordV1 {
    let snapshot = prompt_snapshot();
    let canonical = snapshot.to_canonical_json_bytes().unwrap();
    let attachment = BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &canonical);
    let event = prompt_event();
    let AgentActivityEventV1::PromptPrepared(mut prompt) = event.event else {
        unreachable!();
    };
    prompt.content = Some(CapturedContentV1::Blob {
        blob: attachment.blob.clone(),
    });
    AgentActivityChildRecordV1 {
        frame: AgentActivityFrameV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            occurred_at: event.occurred_at,
            elapsed_ms: event.elapsed_ms,
            scope: event.scope,
            turn: Some(0),
            event: AgentActivityEventV1::PromptPrepared(prompt),
        },
        blobs: vec![attachment],
    }
}

#[test]
fn prompt_snapshot_serialization_is_compact_deterministic_and_ordered() {
    let mut snapshot = prompt_snapshot();
    snapshot.tools.push(PromptToolDefinitionV1 {
        name: "write".into(),
        description: "Write a file.".into(),
        input_schema: json!({"type": "object", "properties": {"content": {"type": "string"}}}),
    });

    let first = snapshot.to_canonical_json_bytes().unwrap();
    let second = snapshot.to_canonical_json_bytes().unwrap();
    assert_eq!(first, second);
    assert!(!first.contains(&b'\n'));
    let reparsed: PromptSnapshotV1 = serde_json::from_slice(&first).unwrap();
    assert_eq!(reparsed, snapshot);
    assert_eq!(reparsed.tools[0].name, "read");
    assert_eq!(reparsed.tools[1].name, "write");

    let tools = snapshot.tools_to_canonical_json_bytes().unwrap();
    let tools_value: Vec<PromptToolDefinitionV1> = serde_json::from_slice(&tools).unwrap();
    assert_eq!(tools_value, snapshot.tools);
    assert!(first.len() > tools.len());
}

#[test]
fn prompt_inline_and_blob_validation_enforces_complete_canonical_snapshots() {
    let event = prompt_event();
    event.validate().expect("canonical inline prompt validates");

    let mut wrong_turn = event.clone();
    wrong_turn.turn = Some(1);
    assert_code(wrong_turn.validate(), ActivityValidationCode::InvalidEvent);

    let mut missing = event.clone();
    let AgentActivityEventV1::PromptPrepared(value) = &mut missing.event else {
        unreachable!();
    };
    value.content = None;
    assert_code(missing.validate(), ActivityValidationCode::InvalidEvent);

    let mut truncated = event.clone();
    let AgentActivityEventV1::PromptPrepared(value) = &mut truncated.event else {
        unreachable!();
    };
    let Some(CapturedContentV1::Inline(inline)) = &mut value.content else {
        unreachable!();
    };
    inline.truncated = true;
    assert_code(truncated.validate(), ActivityValidationCode::InvalidEvent);

    let mut noncanonical = event.clone();
    let AgentActivityEventV1::PromptPrepared(value) = &mut noncanonical.event else {
        unreachable!();
    };
    let Some(CapturedContentV1::Inline(inline)) = &mut value.content else {
        unreachable!();
    };
    inline.text = serde_json::to_string_pretty(&prompt_snapshot()).unwrap();
    value.original_snapshot_bytes = inline.text.len() as u64;
    value.captured_bytes = inline.text.len() as u64;
    assert_code(
        noncanonical.validate(),
        ActivityValidationCode::InvalidEvent,
    );

    let mut unknown_snapshot_field = event.clone();
    let mut snapshot_value = serde_json::to_value(prompt_snapshot()).unwrap();
    snapshot_value["headers"] = json!({"authorization": "forbidden"});
    let AgentActivityEventV1::PromptPrepared(value) = &mut unknown_snapshot_field.event else {
        unreachable!();
    };
    let Some(CapturedContentV1::Inline(inline)) = &mut value.content else {
        unreachable!();
    };
    inline.text = serde_json::to_string(&snapshot_value).unwrap();
    value.original_snapshot_bytes = inline.text.len() as u64;
    value.captured_bytes = inline.text.len() as u64;
    assert_code(
        unknown_snapshot_field.validate(),
        ActivityValidationCode::InvalidEvent,
    );

    let mut wrong_metadata = event.clone();
    let AgentActivityEventV1::PromptPrepared(value) = &mut wrong_metadata.event else {
        unreachable!();
    };
    value.tool_manifest_bytes += 1;
    assert_code(
        wrong_metadata.validate(),
        ActivityValidationCode::InvalidEvent,
    );

    let canonical = prompt_snapshot().to_canonical_json_bytes().unwrap();
    let mut blob = event.clone();
    let AgentActivityEventV1::PromptPrepared(value) = &mut blob.event else {
        unreachable!();
    };
    value.content = Some(CapturedContentV1::Blob {
        blob: BlobReferenceV1::for_bytes(BlobMediaTypeV1::ApplicationJson, &canonical),
    });
    blob.validate()
        .expect("application/json prompt blob validates");
    let AgentActivityEventV1::PromptPrepared(value) = &mut blob.event else {
        unreachable!();
    };
    let Some(CapturedContentV1::Blob { blob: reference }) = &mut value.content else {
        unreachable!();
    };
    reference.media_type = BlobMediaTypeV1::TextPlainUtf8;
    assert_code(blob.validate(), ActivityValidationCode::InvalidEvent);
}

#[test]
fn child_prompt_record_requires_exact_validated_attachments() {
    let record = prompt_child_record();
    record
        .validate()
        .expect("complete attachment-bearing prompt validates");

    let mut missing = record.clone();
    missing.blobs.clear();
    assert_code(
        missing.validate(),
        ActivityValidationCode::BlobReferenceMismatch,
    );

    let mut duplicate = record.clone();
    duplicate.blobs.push(record.blobs[0].clone());
    assert_code(
        duplicate.validate(),
        ActivityValidationCode::BlobReferenceMismatch,
    );

    let mut conflicting = record.clone();
    conflicting.blobs[0].blob.media_type = BlobMediaTypeV1::TextPlainUtf8;
    assert_code(
        conflicting.validate(),
        ActivityValidationCode::BlobReferenceMismatch,
    );

    let mut unreferenced = record.clone();
    unreferenced.blobs.push(BlobAttachmentV1::from_bytes(
        BlobMediaTypeV1::ApplicationJson,
        br#"{"unreferenced":true}"#,
    ));
    assert_code(
        unreferenced.validate(),
        ActivityValidationCode::BlobReferenceMismatch,
    );

    let mut corrupt = record.clone();
    corrupt.blobs[0].data_base64.push('=');
    assert_code(
        corrupt.validate(),
        ActivityValidationCode::InvalidBlobReference,
    );
}

#[test]
fn child_prompt_record_revalidates_decoded_json_and_metadata() {
    let record = prompt_child_record();

    let mut wrong_metadata = record.clone();
    let AgentActivityEventV1::PromptPrepared(prompt) = &mut wrong_metadata.frame.event else {
        unreachable!();
    };
    prompt.initial_user_message_bytes += 1;
    assert_code(
        wrong_metadata.validate(),
        ActivityValidationCode::InvalidEvent,
    );

    let pretty = serde_json::to_vec_pretty(&prompt_snapshot()).unwrap();
    let mut noncanonical = record;
    let attachment = BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &pretty);
    let AgentActivityEventV1::PromptPrepared(prompt) = &mut noncanonical.frame.event else {
        unreachable!();
    };
    prompt.original_snapshot_bytes = pretty.len() as u64;
    prompt.captured_bytes = pretty.len() as u64;
    prompt.content = Some(CapturedContentV1::Blob {
        blob: attachment.blob.clone(),
    });
    noncanonical.blobs = vec![attachment];
    assert_code(
        noncanonical.validate(),
        ActivityValidationCode::InvalidEvent,
    );
}

#[test]
fn child_record_wrapper_is_attachment_only_and_rejects_extensions() {
    let record = prompt_child_record();
    let wire = serde_json::to_value(&record).unwrap();
    assert!(wire.get("frame").is_some());
    assert!(wire.get("blobs").is_some());

    let mut extended = wire;
    extended["headers"] = json!({"authorization": "CREDENTIAL-CHILD-WIRE-SENTINEL"});
    assert!(serde_json::from_value::<AgentActivityChildRecordV1>(extended).is_err());

    let bare = serde_json::to_value(&record.frame).unwrap();
    assert!(bare.get("frame").is_none());
    assert!(bare.get("blobs").is_none());
}

#[test]
fn omitted_prompt_dispositions_have_metadata_only() {
    for disposition in [
        PromptCaptureDispositionV1::OmittedPolicy,
        PromptCaptureDispositionV1::OmittedLimit,
        PromptCaptureDispositionV1::OmittedQuota,
    ] {
        let mut event = prompt_event();
        {
            let AgentActivityEventV1::PromptPrepared(value) = &mut event.event else {
                unreachable!();
            };
            value.disposition = disposition;
            value.captured_bytes = 0;
            value.content = None;
        }
        event.validate().expect("metadata-only prompt validates");

        let AgentActivityEventV1::PromptPrepared(value) = &mut event.event else {
            unreachable!();
        };
        value.captured_bytes = 1;
        assert_code(event.validate(), ActivityValidationCode::InvalidEvent);
    }
}
