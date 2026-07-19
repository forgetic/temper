// SPDX-License-Identifier: MPL-2.0

mod diagnostics;
mod input;
mod validation;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, BlobAttachmentV1, TraceExportRecordV1,
};
use thiserror::Error;

use crate::{
    MetricCoverageV1, RUN_SUMMARY_VERSION, RunIdentityV1, RunMetricsV1, RunSummaryV1,
    RunTerminalStatusV1, RunTerminalV1, TraceCoverageV1, TraceDiagnosticV1, TraceInputKindV1,
};
use diagnostics::content_references;
use input::{
    detect_export, load_journal_attachments, parse_raw_events, read, read_export, read_raw_events,
};
use validation::{finish_normalization, reference_map};

/// One source-independent, fully validated trace stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedTrace {
    pub source: TraceInputKindV1,
    pub events: Vec<AgentRunEventV1>,
    /// Attachments are unique and sorted by their content digest.
    pub attachments: Vec<BlobAttachmentV1>,
    pub diagnostics: Vec<TraceDiagnosticV1>,
}

impl NormalizedTrace {
    /// Serializes records in the same deterministic order as the engine export:
    /// each event is followed by its newly referenced attachments, sorted by
    /// digest. Every record ends in one newline.
    pub fn canonical_export(&self) -> Result<Vec<u8>, TraceIngestError> {
        let attachments = self
            .attachments
            .iter()
            .map(|attachment| (attachment.blob.digest.as_str(), attachment))
            .collect::<BTreeMap<_, _>>();
        let mut emitted = BTreeSet::new();
        let mut bytes = Vec::new();
        for event in &self.events {
            write_record(&mut bytes, &TraceExportRecordV1::event(event.clone()))?;
            let mut references = content_references(event);
            references.sort_by(|left, right| left.digest.cmp(&right.digest));
            for reference in references {
                if !emitted.insert(reference.digest.clone()) {
                    continue;
                }
                let attachment = attachments.get(reference.digest.as_str()).ok_or_else(|| {
                    TraceIngestError::Attachment(format!(
                        "canonical stream is missing attachment {}",
                        reference.digest
                    ))
                })?;
                write_record(
                    &mut bytes,
                    &TraceExportRecordV1::attachment((*attachment).clone()),
                )?;
            }
        }
        Ok(bytes)
    }

    /// Builds the stable base run-summary contract. Metric extraction augments
    /// the optional groups without converting unavailable observations to zero.
    pub fn run_summary(&self) -> RunSummaryV1 {
        let first = self
            .events
            .first()
            .expect("normalized traces always contain an event");
        let terminal = self.events.iter().find_map(|event| match &event.event {
            AgentActivityEventV1::RunFinished(finished) => Some(RunTerminalV1 {
                status: match finished.status {
                    temper_protocol_activity::RunStatusV1::Succeeded => {
                        RunTerminalStatusV1::Succeeded
                    }
                    temper_protocol_activity::RunStatusV1::Cancelled => {
                        RunTerminalStatusV1::Cancelled
                    }
                },
                duration_ms: Some(finished.duration_ms),
                stop_reason: finished.stop_reason,
                failure: None,
            }),
            AgentActivityEventV1::RunFailed(failed) => Some(RunTerminalV1 {
                status: RunTerminalStatusV1::Failed,
                duration_ms: None,
                stop_reason: None,
                failure: Some(failed.failure.clone()),
            }),
            _ => None,
        });
        let capture = self.events.iter().find_map(|event| match event.event {
            AgentActivityEventV1::RunStarted(ref started) => Some(started.capture),
            _ => None,
        });
        let first_seq = self.events.first().map(|event| event.seq);
        let last_seq = self.events.last().map(|event| event.seq);
        let referenced = reference_map(&self.events)
            .expect("normalized trace references were already validated");

        RunSummaryV1 {
            version: RUN_SUMMARY_VERSION,
            identity: RunIdentityV1 {
                run_id: first.run_id.clone(),
                assignment: first.assignment.clone(),
                agent_session_id: first.agent_session_id.clone(),
            },
            source: self.source,
            capture,
            trace: TraceCoverageV1 {
                events: MetricCoverageV1 {
                    observed: self.events.len() as u64,
                    expected: last_seq,
                },
                attachments: MetricCoverageV1 {
                    observed: self.attachments.len() as u64,
                    expected: Some(referenced.len() as u64),
                },
                first_seq,
                last_seq,
                terminal_event_observed: terminal.is_some(),
            },
            wall_time_ms: terminal.as_ref().and_then(|value| value.duration_ms),
            terminal,
            metrics: RunMetricsV1::default(),
            validation: None,
            diff: None,
            host: None,
            diagnostics: self.diagnostics.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum TraceIngestError {
    #[error("{operation} {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("trace input {} contains no complete event records", path.display())]
    Empty { path: PathBuf },
    #[error("invalid trace record at {}:{line}: {detail}", path.display())]
    InvalidRecord {
        path: PathBuf,
        line: usize,
        detail: String,
    },
    #[error("invalid activity event at sequence {seq}: {detail}")]
    InvalidActivity { seq: u64, detail: String },
    #[error("invalid trace stream: {0}")]
    InvalidStream(String),
    #[error("invalid trace attachment: {0}")]
    Attachment(String),
    #[error("serialize canonical trace export: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Detects a journal run directory, bare event JSONL, or versioned export
/// JSONL and normalizes it into one canonical event/attachment stream.
pub fn ingest_trace(path: impl AsRef<Path>) -> Result<NormalizedTrace, TraceIngestError> {
    let path = path.as_ref();
    if path.is_dir() {
        let (events, mut diagnostics) = read_raw_events(&path.join("events.jsonl"))?;
        let attachments = load_journal_attachments(path, &events)?;
        return finish_normalization(
            TraceInputKindV1::JournalDirectory,
            events,
            attachments,
            &mut diagnostics,
        );
    }

    let bytes = read(path, "read trace input")?;
    if detect_export(path, &bytes)? {
        let (events, attachments, mut diagnostics) = read_export(path, &bytes)?;
        finish_normalization(
            TraceInputKindV1::ExportJsonl,
            events,
            attachments,
            &mut diagnostics,
        )
    } else {
        let (events, mut diagnostics) = parse_raw_events(path, &bytes)?;
        let run_directory = path.parent().unwrap_or_else(|| Path::new("."));
        let attachments = load_journal_attachments(run_directory, &events)?;
        finish_normalization(
            TraceInputKindV1::RawEventsJsonl,
            events,
            attachments,
            &mut diagnostics,
        )
    }
}

/// Writes a normalized deterministic export without retaining source-specific
/// journal metadata.
pub fn write_canonical_export(
    trace: &NormalizedTrace,
    path: impl AsRef<Path>,
) -> Result<(), TraceIngestError> {
    let path = path.as_ref();
    fs::write(path, trace.canonical_export()?).map_err(|source| TraceIngestError::Io {
        operation: "write canonical trace export",
        path: path.to_path_buf(),
        source,
    })
}

fn write_record(
    bytes: &mut Vec<u8>,
    record: &TraceExportRecordV1,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer(&mut *bytes, record)?;
    bytes.push(b'\n');
    Ok(())
}
