//! Best-effort operator-local capture of bounded model-visible graph results.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::sync::Mutex;

use temper_agent_core::OperatorTranscriptSink;
use temper_protocol_activity::{
    CaptureModeV1, InlineContentV1, MAX_OPERATOR_TRANSCRIPT_BYTES, MAX_OPERATOR_TRANSCRIPT_RECORDS,
    OPERATOR_TRANSCRIPT_RECORD_VERSION, OperatorTranscriptToolResultV1,
};

/// This writer is deliberately outside the activity projection set: records
/// never receive assignment identity, sequence numbers, or durable transport.
pub(super) struct OperatorTranscriptCapture {
    state: Mutex<CaptureState>,
}

struct CaptureState {
    writer: BufWriter<std::fs::File>,
    records: usize,
    bytes: usize,
}

impl OperatorTranscriptCapture {
    pub(super) fn open(mode: CaptureModeV1, path: Option<&Path>) -> Option<Self> {
        if mode != CaptureModeV1::Diagnostic {
            return None;
        }
        let path = path?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(path).ok()?;
        Some(Self {
            state: Mutex::new(CaptureState {
                writer: BufWriter::new(file),
                records: 0,
                bytes: 0,
            }),
        })
    }
}

impl OperatorTranscriptSink for OperatorTranscriptCapture {
    fn graph_result(&self, call_id: &str, tool_name: &str, text: &str, truncated: bool) {
        if !tool_name.starts_with("codebase_memory_") || text.is_empty() {
            return;
        }
        let record = OperatorTranscriptToolResultV1 {
            version: OPERATOR_TRANSCRIPT_RECORD_VERSION,
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            model_result_text: InlineContentV1 {
                text: text.to_string(),
                truncated,
            },
        };
        if record.validate().is_err() {
            return;
        }
        let Ok(mut encoded) = serde_json::to_vec(&record) else {
            return;
        };
        encoded.push(b'\n');
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.records >= MAX_OPERATOR_TRANSCRIPT_RECORDS
            || state.bytes.saturating_add(encoded.len()) > MAX_OPERATOR_TRANSCRIPT_BYTES
        {
            return;
        }
        if state.writer.write_all(&encoded).is_ok() {
            let _ = state.writer.flush();
            state.records += 1;
            state.bytes += encoded.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_capture_keeps_only_explicit_bounded_graph_results() {
        let temporary = tempfile::tempdir().expect("temporary capture dir");
        let path = temporary.path().join("operator-transcript.jsonl");
        let capture = OperatorTranscriptCapture::open(CaptureModeV1::Diagnostic, Some(&path))
            .expect("diagnostic capture opens");
        capture.graph_result(
            "graph-call",
            "codebase_memory_search_graph",
            "cold stable upsert is ready",
            false,
        );
        capture.graph_result(
            "not-graph",
            "read",
            "Authorization: Bearer MCP-FIXTURE-SECRET",
            false,
        );
        drop(capture);

        let wire = std::fs::read_to_string(path).expect("capture is readable");
        assert!(wire.contains("cold stable upsert is ready"));
        assert!(!wire.contains("MCP-FIXTURE-SECRET"));
        let record: OperatorTranscriptToolResultV1 =
            serde_json::from_str(wire.trim()).expect("capture record parses");
        record.validate().expect("capture record validates");
    }

    #[test]
    fn less_permissive_capture_modes_never_create_operator_transcript() {
        let temporary = tempfile::tempdir().expect("temporary capture dir");
        for mode in [
            CaptureModeV1::Off,
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
        ] {
            let path = temporary.path().join(format!("{mode:?}.jsonl"));
            assert!(OperatorTranscriptCapture::open(mode, Some(&path)).is_none());
            assert!(!path.exists());
        }
    }

    #[test]
    fn capture_is_create_only_and_has_fixed_total_bounds() {
        let temporary = tempfile::tempdir().expect("temporary capture dir");
        let path = temporary.path().join("operator-transcript.jsonl");
        std::fs::write(&path, "preexisting private data").expect("write preexisting file");
        assert!(OperatorTranscriptCapture::open(CaptureModeV1::Diagnostic, Some(&path)).is_none());
        assert_eq!(
            std::fs::read_to_string(&path).expect("preexisting file remains"),
            "preexisting private data"
        );

        let bounded = temporary.path().join("bounded.jsonl");
        let capture = OperatorTranscriptCapture::open(CaptureModeV1::Diagnostic, Some(&bounded))
            .expect("bounded capture opens");
        for index in 0..(MAX_OPERATOR_TRANSCRIPT_RECORDS + 2) {
            capture.graph_result(
                &format!("graph-{index}"),
                "codebase_memory_search_graph",
                "safe",
                false,
            );
        }
        drop(capture);
        let bytes = std::fs::read(&bounded).expect("bounded capture readable");
        assert!(bytes.len() <= MAX_OPERATOR_TRANSCRIPT_BYTES);
        assert_eq!(
            bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            MAX_OPERATOR_TRANSCRIPT_RECORDS
        );
    }
}
