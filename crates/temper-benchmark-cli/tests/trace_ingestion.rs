// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    RUN_SUMMARY_VERSION, RunSummaryV1, TraceDiagnosticCodeV1, TraceIngestError, TraceInputKindV1,
    ingest_trace,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn journal_and_export_normalize_to_the_same_canonical_stream() {
    let journal = ingest_trace(fixture("journal-complete")).expect("journal ingests");
    let export = ingest_trace(fixture("complete-export.jsonl")).expect("export ingests");

    assert_eq!(journal.source, TraceInputKindV1::JournalDirectory);
    assert_eq!(export.source, TraceInputKindV1::ExportJsonl);
    assert_eq!(journal.events, export.events);
    assert_eq!(journal.attachments, export.attachments);
    assert_eq!(
        journal.canonical_export().unwrap(),
        export.canonical_export().unwrap()
    );
    assert_eq!(journal.attachments.len(), 1);

    let temporary = tempfile::tempdir().unwrap();
    let canonical = temporary.path().join("canonical.jsonl");
    fs::write(&canonical, journal.canonical_export().unwrap()).unwrap();
    let reingested = ingest_trace(canonical).expect("canonical export re-ingests");
    assert_eq!(journal.events, reingested.events);
    assert_eq!(journal.attachments, reingested.attachments);
    assert_eq!(
        journal.canonical_export().unwrap(),
        reingested.canonical_export().unwrap()
    );
}

#[test]
fn partial_stream_records_gaps_truncation_incomplete_calls_and_missing_terminal() {
    let trace = ingest_trace(fixture("partial-events.jsonl")).expect("partial stream is useful");
    let codes = trace
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    for expected in [
        TraceDiagnosticCodeV1::TruncatedRecord,
        TraceDiagnosticCodeV1::SequenceGap,
        TraceDiagnosticCodeV1::IncompleteModelCall,
        TraceDiagnosticCodeV1::MissingTerminalEvent,
        TraceDiagnosticCodeV1::HostEvidenceUnavailable,
        TraceDiagnosticCodeV1::DiffEvidenceUnavailable,
        TraceDiagnosticCodeV1::ValidationEvidenceUnavailable,
    ] {
        assert!(codes.contains(&expected), "missing diagnostic {expected:?}");
    }

    let summary = trace.run_summary();
    assert_eq!(summary.version, RUN_SUMMARY_VERSION);
    assert_eq!(summary.trace.events.observed, 2);
    assert_eq!(summary.trace.events.expected, Some(3));
    assert!(!summary.trace.terminal_event_observed);
    assert!(summary.terminal.is_none());
    assert!(summary.wall_time_ms.is_none());
    assert!(summary.metrics.model.is_none());
    assert!(summary.diff.is_none());
    assert!(summary.host.is_none());
}

#[test]
fn malformed_identity_sequence_scope_and_export_version_are_rejected() {
    let cases = [
        ("malformed-identity.jsonl", "assignment identity changed"),
        ("malformed-sequence.jsonl", "does not increase"),
        ("malformed-scope.jsonl", "missing parent"),
        (
            "unsupported-export-version.jsonl",
            "unsupported trace export record version 2",
        ),
        ("unsupported-event-version.jsonl", "invalid event.version"),
    ];

    for (name, expected) in cases {
        let error = ingest_trace(fixture(name)).expect_err(name);
        assert!(
            error.to_string().contains(expected),
            "{name}: expected {expected:?}, got {error}"
        );
    }
}

#[test]
fn missing_and_corrupt_journal_attachments_are_rejected() {
    let source = fixture("journal-complete");
    let digest = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

    let missing = tempfile::tempdir().unwrap();
    fs::write(
        missing.path().join("events.jsonl"),
        fs::read(source.join("events.jsonl")).unwrap(),
    )
    .unwrap();
    let error = ingest_trace(missing.path()).expect_err("missing blob fails");
    assert!(matches!(error, TraceIngestError::Attachment(_)));
    assert!(error.to_string().contains("missing journal attachment"));

    let corrupt = tempfile::tempdir().unwrap();
    fs::create_dir(corrupt.path().join("blobs")).unwrap();
    fs::write(
        corrupt.path().join("events.jsonl"),
        fs::read(source.join("events.jsonl")).unwrap(),
    )
    .unwrap();
    fs::write(corrupt.path().join("blobs").join(digest), b"not-json").unwrap();
    let error = ingest_trace(corrupt.path()).expect_err("corrupt blob fails");
    assert!(error.to_string().contains("content-address validation"));
}

#[test]
fn run_summary_rejects_unknown_fields_and_unsupported_versions() {
    let trace = ingest_trace(fixture("journal-complete")).unwrap();
    let summary = trace.run_summary();
    let rendered = serde_json::to_value(&summary).unwrap();

    assert!(rendered.get("host").is_none());
    assert!(rendered.get("diff").is_none());
    assert!(rendered["metrics"].as_object().unwrap().is_empty());
    assert_eq!(
        serde_json::from_value::<RunSummaryV1>(rendered.clone()).unwrap(),
        summary
    );

    let mut unsupported = rendered.clone();
    unsupported["version"] = serde_json::json!(2);
    let error = serde_json::from_value::<RunSummaryV1>(unsupported).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported run summary version 2")
    );

    let mut unknown = rendered;
    unknown["surprise"] = serde_json::json!(true);
    let error = serde_json::from_value::<RunSummaryV1>(unknown).unwrap_err();
    assert!(error.to_string().contains("unknown field `surprise`"));
}
