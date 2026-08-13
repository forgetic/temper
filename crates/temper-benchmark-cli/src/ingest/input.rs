// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::io;
use std::path::Path;

use temper_protocol_activity::{AgentRunEventV1, BlobAttachmentV1, TraceExportRecordV1};

use super::TraceIngestError;
use super::diagnostics::warning;
use super::validation::reference_map;
use crate::{TraceDiagnosticCodeV1, TraceDiagnosticV1};

type ParsedExport = (
    Vec<AgentRunEventV1>,
    Vec<BlobAttachmentV1>,
    Vec<temper_protocol_activity::OperatorTranscriptToolResultV1>,
    Vec<TraceDiagnosticV1>,
);

pub(super) fn read_raw_events(
    path: &Path,
) -> Result<(Vec<AgentRunEventV1>, Vec<TraceDiagnosticV1>), TraceIngestError> {
    let bytes = read(path, "read journal events")?;
    parse_raw_events(path, &bytes)
}

pub(super) fn parse_raw_events(
    path: &Path,
    bytes: &[u8],
) -> Result<(Vec<AgentRunEventV1>, Vec<TraceDiagnosticV1>), TraceIngestError> {
    let mut events = Vec::new();
    let mut diagnostics = Vec::new();
    visit_lines(bytes, &mut diagnostics, |line, value| {
        let event =
            serde_json::from_slice(value).map_err(|error| TraceIngestError::InvalidRecord {
                path: path.to_path_buf(),
                line,
                detail: error.to_string(),
            })?;
        events.push(event);
        Ok(())
    })?;
    require_events(path, events).map(|events| (events, diagnostics))
}

pub(super) fn read_export(path: &Path, bytes: &[u8]) -> Result<ParsedExport, TraceIngestError> {
    let mut events = Vec::new();
    let mut attachments = Vec::new();
    let mut operator_transcript = Vec::new();
    let mut diagnostics = Vec::new();
    visit_lines(bytes, &mut diagnostics, |line, value| {
        let record: TraceExportRecordV1 =
            serde_json::from_slice(value).map_err(|error| TraceIngestError::InvalidRecord {
                path: path.to_path_buf(),
                line,
                detail: error.to_string(),
            })?;
        match record {
            TraceExportRecordV1::AgentRunEventV1 { event, .. } => events.push(event),
            TraceExportRecordV1::BlobAttachmentV1 { attachment, .. } => {
                attachments.push(attachment);
            }
            TraceExportRecordV1::OperatorTranscriptV1 { record, .. } => {
                operator_transcript.push(record);
            }
        }
        Ok(())
    })?;
    require_events(path, events)
        .map(|events| (events, attachments, operator_transcript, diagnostics))
}

pub(super) fn detect_export(path: &Path, bytes: &[u8]) -> Result<bool, TraceIngestError> {
    let Some(line) = complete_nonempty_lines(bytes).next() else {
        return Err(TraceIngestError::Empty {
            path: path.to_path_buf(),
        });
    };
    let value: serde_json::Value =
        serde_json::from_slice(line.bytes).map_err(|error| TraceIngestError::InvalidRecord {
            path: path.to_path_buf(),
            line: line.number,
            detail: error.to_string(),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| TraceIngestError::InvalidRecord {
            path: path.to_path_buf(),
            line: line.number,
            detail: "record must be a JSON object".to_string(),
        })?;
    Ok(object.contains_key("type") && object.contains_key("version"))
}

pub(super) fn load_journal_attachments(
    run_directory: &Path,
    events: &[AgentRunEventV1],
) -> Result<Vec<BlobAttachmentV1>, TraceIngestError> {
    let references = reference_map(events)?;
    let mut attachments = Vec::with_capacity(references.len());
    for reference in references.values() {
        let digest = reference.digest.strip_prefix("sha256:").ok_or_else(|| {
            TraceIngestError::Attachment("blob digest has no sha256 prefix".to_string())
        })?;
        let path = run_directory.join("blobs").join(digest);
        let bytes = fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                TraceIngestError::Attachment(format!(
                    "missing journal attachment {} at {}",
                    reference.digest,
                    path.display()
                ))
            } else {
                TraceIngestError::Io {
                    operation: "read journal attachment",
                    path: path.clone(),
                    source,
                }
            }
        })?;
        let attachment = BlobAttachmentV1::from_bytes(reference.media_type, &bytes);
        if attachment.blob != *reference {
            return Err(TraceIngestError::Attachment(format!(
                "journal attachment {} fails content-address validation",
                reference.digest
            )));
        }
        attachments.push(attachment);
    }
    Ok(attachments)
}

pub(super) fn read(path: &Path, operation: &'static str) -> Result<Vec<u8>, TraceIngestError> {
    fs::read(path).map_err(|source| TraceIngestError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

fn require_events(
    path: &Path,
    events: Vec<AgentRunEventV1>,
) -> Result<Vec<AgentRunEventV1>, TraceIngestError> {
    if events.is_empty() {
        Err(TraceIngestError::Empty {
            path: path.to_path_buf(),
        })
    } else {
        Ok(events)
    }
}

struct JsonLine<'a> {
    number: usize,
    bytes: &'a [u8],
    terminated: bool,
}

fn lines(bytes: &[u8]) -> Vec<JsonLine<'_>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut number = 1;
    while start < bytes.len() {
        let newline = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| start + offset);
        let (end, terminated) = newline.map_or((bytes.len(), false), |end| (end, true));
        let mut value = &bytes[start..end];
        if value.ends_with(b"\r") {
            value = &value[..value.len() - 1];
        }
        result.push(JsonLine {
            number,
            bytes: value,
            terminated,
        });
        if !terminated {
            break;
        }
        start = end + 1;
        number += 1;
    }
    result
}

fn complete_nonempty_lines(bytes: &[u8]) -> impl Iterator<Item = JsonLine<'_>> {
    lines(bytes)
        .into_iter()
        .filter(|line| !line.bytes.iter().all(u8::is_ascii_whitespace))
        .filter(|line| {
            line.terminated || serde_json::from_slice::<serde_json::Value>(line.bytes).is_ok()
        })
}

fn visit_lines(
    bytes: &[u8],
    diagnostics: &mut Vec<TraceDiagnosticV1>,
    mut visit: impl FnMut(usize, &[u8]) -> Result<(), TraceIngestError>,
) -> Result<(), TraceIngestError> {
    for line in lines(bytes) {
        if line.bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if !line.terminated && serde_json::from_slice::<serde_json::Value>(line.bytes).is_err() {
            diagnostics.push(warning(
                TraceDiagnosticCodeV1::TruncatedRecord,
                format!(
                    "ignored an incomplete final JSONL record at line {}",
                    line.number
                ),
                None,
            ));
            break;
        }
        visit(line.number, line.bytes)?;
    }
    Ok(())
}
