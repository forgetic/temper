use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use temper_agent_core::{AgentEvent, ModelIdentity};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityChildRecordV1, AgentActivityEventV1, CaptureModeV1,
    CapturedContentV1, PromptCaptureDispositionV1, PromptSnapshotV1, PromptToolDefinitionV1,
};
use tongs::provider::ToolDef;

use super::*;

struct PromptClock {
    values: Mutex<VecDeque<ActivityTimestamp>>,
}

impl PromptClock {
    fn new(elapsed: impl IntoIterator<Item = u64>) -> Self {
        Self {
            values: Mutex::new(
                elapsed
                    .into_iter()
                    .map(|elapsed_ms| ActivityTimestamp {
                        occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
                        elapsed_ms,
                    })
                    .collect(),
            ),
        }
    }
}

impl ActivityClock for PromptClock {
    fn now(&self) -> ActivityTimestamp {
        self.values
            .lock()
            .expect("clock")
            .pop_front()
            .unwrap_or(ActivityTimestamp {
                occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
                elapsed_ms: 999,
            })
    }
}

#[derive(Default)]
struct RecordRecorder(Mutex<Vec<AgentActivityChildRecordV1>>);

impl ActivityProjection for RecordRecorder {
    fn emit(&self, record: &AgentActivityChildRecordV1) {
        self.0.lock().expect("records").push(record.clone());
    }
}

fn prompt_source() -> (Option<String>, String, Vec<ToolDef>) {
    (
        Some("system café password=hunter2".to_string()),
        "initial 🙂 message".to_string(),
        vec![
            ToolDef {
                name: "second".to_string(),
                description: "second description".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"z": {"type": "string"}}
                }),
            },
            ToolDef {
                name: "first".to_string(),
                description: "first description".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"a": {"type": "integer"}},
                    "required": ["a"]
                }),
            },
        ],
    )
}

fn emit_prompt(sink: &Arc<dyn temper_agent_core::EventSink>) {
    let (system_prompt, initial_user_message, tools) = prompt_source();
    sink.emit(AgentEvent::PromptPrepared {
        system_prompt,
        initial_user_message,
        tools,
    });
}

fn expected_prompt_snapshot() -> PromptSnapshotV1 {
    let (system_prompt, initial_user_message, tools) = prompt_source();
    PromptSnapshotV1 {
        system_prompt,
        initial_user_message,
        tools: tools
            .into_iter()
            .map(|tool| PromptToolDefinitionV1 {
                name: tool.name,
                description: tool.description,
                input_schema: tool.parameters,
            })
            .collect(),
    }
}

#[test]
fn prompt_capture_modes_preserve_exact_snapshot_metadata_and_order() {
    let expected = expected_prompt_snapshot();
    let canonical = expected.to_canonical_json_bytes().unwrap();
    let tool_manifest = expected.tools_to_canonical_json_bytes().unwrap();

    for mode in [
        CaptureModeV1::Off,
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(RecordRecorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                ..Default::default()
            },
            Arc::new(PromptClock::new(0..10)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        let sink = run.observability.events;
        emit_prompt(&sink);
        sink.emit(AgentEvent::TurnStart { turn: 0 });

        let records = recorder.0.lock().expect("records");
        let kinds = records
            .iter()
            .map(|record| record.frame.event.event_type())
            .collect::<Vec<_>>();
        if mode == CaptureModeV1::Off {
            assert_eq!(kinds, ["scope.started", "turn.started"]);
            let wire = serde_json::to_string(&*records).unwrap();
            assert!(!wire.contains("password=hunter2"));
            continue;
        }
        assert_eq!(kinds, ["scope.started", "prompt.prepared", "turn.started"]);
        let prompt_record = &records[1];
        assert_eq!(prompt_record.frame.turn, Some(0));
        let AgentActivityEventV1::PromptPrepared(prompt) = &prompt_record.frame.event else {
            panic!("prompt.prepared");
        };
        assert_eq!(
            prompt.system_prompt_bytes,
            expected.system_prompt.as_ref().unwrap().len() as u64
        );
        assert_eq!(
            prompt.initial_user_message_bytes,
            expected.initial_user_message.len() as u64
        );
        assert_eq!(prompt.tool_manifest_bytes, tool_manifest.len() as u64);
        assert_eq!(prompt.tool_count, 2);
        assert_eq!(prompt.original_snapshot_bytes, canonical.len() as u64);

        if mode == CaptureModeV1::Metadata {
            assert_eq!(
                prompt.disposition,
                PromptCaptureDispositionV1::OmittedPolicy
            );
            assert_eq!(prompt.captured_bytes, 0);
            assert!(prompt.content.is_none());
            assert!(prompt_record.blobs.is_empty());
            let wire = serde_json::to_string(prompt_record).unwrap();
            assert!(!wire.contains("password=hunter2"));
            assert!(!wire.contains("input_schema"));
        } else {
            assert_eq!(prompt.disposition, PromptCaptureDispositionV1::Captured);
            assert_eq!(prompt.captured_bytes, canonical.len() as u64);
            let Some(CapturedContentV1::Inline(inline)) = &prompt.content else {
                panic!("small prompt should be inline");
            };
            assert_eq!(inline.text.as_bytes(), canonical);
            assert!(!inline.truncated);
            assert!(inline.text.contains("password=hunter2"));
            let decoded: PromptSnapshotV1 = serde_json::from_str(&inline.text).unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(decoded.tools[0].name, "second");
            assert_eq!(decoded.tools[1].name, "first");
            assert!(prompt_record.blobs.is_empty());
        }
    }
}

#[test]
fn prompt_inline_blob_and_over_limit_boundaries_are_exact() {
    let expected = expected_prompt_snapshot();
    let canonical = expected.to_canonical_json_bytes().unwrap();
    assert!(canonical.len() > 1);

    let cases = [
        (canonical.len(), canonical.len() as u64, "inline"),
        (canonical.len() - 1, canonical.len() as u64, "blob"),
        (canonical.len() - 1, canonical.len() as u64 - 1, "omitted"),
    ];
    for (max_inline_bytes, max_blob_bytes, expected_storage) in cases {
        let recorder = Arc::new(RecordRecorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: CaptureModeV1::Transcript,
                max_inline_bytes: u32::try_from(max_inline_bytes).unwrap(),
                max_blob_bytes,
                ..Default::default()
            },
            Arc::new(PromptClock::new(0..10)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        emit_prompt(&run.observability.events);

        let records = recorder.0.lock().expect("records");
        let record = records
            .iter()
            .find(|record| matches!(&record.frame.event, AgentActivityEventV1::PromptPrepared(_)))
            .expect("prompt record");
        let AgentActivityEventV1::PromptPrepared(prompt) = &record.frame.event else {
            unreachable!();
        };
        match expected_storage {
            "inline" => {
                let Some(CapturedContentV1::Inline(inline)) = &prompt.content else {
                    panic!("inline boundary should capture inline");
                };
                assert_eq!(inline.text.as_bytes(), canonical);
                assert!(record.blobs.is_empty());
            }
            "blob" => {
                let Some(CapturedContentV1::Blob { blob }) = &prompt.content else {
                    panic!("blob boundary should capture a reference");
                };
                assert_eq!(blob.bytes, canonical.len() as u64);
                assert_eq!(record.blobs.len(), 1);
                assert_eq!(record.blobs[0].blob, *blob);
                assert_eq!(record.blobs[0].decode().unwrap(), canonical);
                record.validate().expect("complete child record validates");
            }
            "omitted" => {
                assert_eq!(prompt.disposition, PromptCaptureDispositionV1::OmittedLimit);
                assert_eq!(prompt.original_snapshot_bytes, canonical.len() as u64);
                assert_eq!(prompt.captured_bytes, 0);
                assert!(prompt.content.is_none());
                assert!(record.blobs.is_empty());
            }
            _ => unreachable!(),
        }
    }
}
