// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    RUN_SUMMARY_VERSION, RunSummaryV1, TraceDiagnosticCodeV1, TraceIngestError, TraceInputKindV1,
    aggregate_run_summaries, ingest_trace,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, InlineContentV1, OPERATOR_TRANSCRIPT_RECORD_VERSION,
    OperatorTranscriptToolResultV1,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn write_events(path: &Path, events: &[AgentRunEventV1]) {
    let mut jsonl = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    jsonl.push('\n');
    fs::write(path, jsonl).unwrap();
}

fn insert_duplicate(events: &mut Vec<AgentRunEventV1>, index: usize) {
    let mut duplicate = events[index].clone();
    duplicate.seq += 1;
    duplicate.elapsed_ms += 1;
    for event in &mut events[index + 1..] {
        event.seq += 1;
        event.elapsed_ms += 1;
    }
    events.insert(index + 1, duplicate);
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
fn operator_transcript_is_local_export_only_and_never_enters_summaries_or_aggregates() {
    const READINESS: &str = "cold stable upsert is ready";
    let mut trace = ingest_trace(fixture("complete-export.jsonl")).expect("export ingests");
    trace.operator_transcript = vec![OperatorTranscriptToolResultV1 {
        version: OPERATOR_TRANSCRIPT_RECORD_VERSION,
        call_id: "graph-read".to_string(),
        tool_name: "codebase_memory_search_graph".to_string(),
        model_result_text: InlineContentV1 {
            text: READINESS.to_string(),
            truncated: false,
        },
    }];

    let local_export = String::from_utf8(trace.canonical_export().unwrap()).unwrap();
    assert!(local_export.contains(READINESS));
    assert!(
        !serde_json::to_string(&trace.events)
            .unwrap()
            .contains(READINESS)
    );

    let summary = trace.run_summary();
    assert!(!serde_json::to_string(&summary).unwrap().contains(READINESS));
    let aggregate = aggregate_run_summaries([summary]).expect("summary aggregates");
    assert!(
        !serde_json::to_string(&aggregate)
            .unwrap()
            .contains(READINESS)
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
    let model = summary.metrics.model.as_ref().unwrap();
    assert_eq!(summary.metrics.turns, Some(1));
    assert_eq!(model.calls, 1);
    assert_eq!(model.attempts, 1);
    assert_eq!(model.cumulative_duration_ms, None);
    assert_eq!(model.duration_coverage.observed, 0);
    assert_eq!(model.duration_coverage.expected, Some(1));
    assert_eq!(summary.metrics.tools.as_ref().unwrap().calls, 0);
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
fn parallel_child_scopes_may_reuse_model_and_tool_call_ids() {
    let trace =
        ingest_trace(fixture("parallel-child-scopes.jsonl")).expect("parallel scoped calls ingest");
    assert!(!trace.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.code,
        TraceDiagnosticCodeV1::IncompleteModelCall | TraceDiagnosticCodeV1::IncompleteToolCall
    )));

    let summary = trace.run_summary();
    assert_eq!(summary.metrics.turns, Some(2));
    let model = summary.metrics.model.as_ref().unwrap();
    assert_eq!(
        (model.calls, model.attempts, model.succeeded_attempts),
        (2, 2, 2)
    );
    let tools = summary.metrics.tools.as_ref().unwrap();
    assert_eq!((tools.calls, tools.succeeded), (2, 2));
    assert_eq!(tools.by_name["read"].calls, 1);
    assert_eq!(tools.by_name["write"].calls, 1);
}

#[test]
fn incomplete_parallel_calls_are_reported_only_for_their_scope() {
    let trace = ingest_trace(fixture("parallel-child-scopes-incomplete.jsonl"))
        .expect("incomplete scoped calls remain analyzable");
    let incomplete = trace
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.code,
                TraceDiagnosticCodeV1::IncompleteModelCall
                    | TraceDiagnosticCodeV1::IncompleteToolCall
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(incomplete.len(), 2);
    assert!(
        incomplete
            .iter()
            .all(|diagnostic| diagnostic.message.contains("scope child-b"))
    );
    assert!(
        incomplete
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("child-a"))
    );
    assert!(incomplete.iter().any(|diagnostic| {
        diagnostic.code == TraceDiagnosticCodeV1::IncompleteModelCall && diagnostic.seq == Some(5)
    }));
    assert!(incomplete.iter().any(|diagnostic| {
        diagnostic.code == TraceDiagnosticCodeV1::IncompleteToolCall && diagnostic.seq == Some(7)
    }));

    let summary = trace.run_summary();
    assert_eq!(summary.metrics.turns, Some(2));
    let model = summary.metrics.model.as_ref().unwrap();
    assert_eq!((model.calls, model.attempts), (2, 2));
    assert_eq!(
        (
            model.duration_coverage.observed,
            model.duration_coverage.expected
        ),
        (1, Some(2))
    );
    let tools = summary.metrics.tools.as_ref().unwrap();
    assert_eq!(tools.calls, 2);
    assert_eq!(
        (
            tools.duration_coverage.observed,
            tools.duration_coverage.expected
        ),
        (1, Some(2))
    );
}

#[test]
fn duplicate_and_mismatched_calls_within_one_scope_are_rejected() {
    let base = ingest_trace(fixture("parallel-child-scopes.jsonl")).unwrap();

    for (is_target, expected) in [
        (
            (|event: &AgentRunEventV1| {
                event.scope.id == "child-a"
                    && matches!(event.event, AgentActivityEventV1::ModelCallStarted(_))
            }) as fn(&AgentRunEventV1) -> bool,
            "model call turn-0 attempt 0 in scope child-a starts more than once",
        ),
        (
            (|event: &AgentRunEventV1| {
                event.scope.id == "child-a"
                    && matches!(event.event, AgentActivityEventV1::ToolStarted(_))
            }) as fn(&AgentRunEventV1) -> bool,
            "tool call tool-0 in scope child-a starts more than once",
        ),
    ] {
        let mut events = base.events.clone();
        let index = events.iter().position(is_target).unwrap();
        insert_duplicate(&mut events, index);
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("events.jsonl");
        write_events(&path, &events);
        let error = ingest_trace(path).expect_err("same-scope duplicate must fail");
        assert!(error.to_string().contains(expected), "{error}");
    }

    let mut events = base.events;
    let finish = events
        .iter_mut()
        .find(|event| {
            event.scope.id == "child-a"
                && matches!(event.event, AgentActivityEventV1::ToolFinished(_))
        })
        .unwrap();
    let AgentActivityEventV1::ToolFinished(call) = &mut finish.event else {
        unreachable!();
    };
    call.name = "write".to_string();
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("events.jsonl");
    write_events(&path, &events);
    let error = ingest_trace(path).expect_err("same-scope mismatch must fail");
    assert!(
        error
            .to_string()
            .contains("tool call tool-0 in scope child-a changes name from read to write")
    );
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
    let metrics = rendered["metrics"].as_object().unwrap();
    assert_eq!(metrics["turns"], serde_json::json!(1));
    assert_eq!(metrics["model"]["calls"], serde_json::json!(0));
    assert_eq!(metrics["tools"]["calls"], serde_json::json!(1));
    assert_eq!(metrics["tools"]["ordinary"]["calls"], serde_json::json!(1));
    assert_eq!(metrics["structure"]["mutations"], serde_json::json!(0));
    assert_eq!(metrics["structure"]["mutation_turns"], serde_json::json!(0));
    assert_eq!(
        metrics["structure"]["single_mutation_turns"],
        serde_json::json!(0)
    );
    assert_eq!(
        metrics["structure"]["max_mutations_per_turn"],
        serde_json::json!(0)
    );
    assert_eq!(
        serde_json::from_value::<RunSummaryV1>(rendered.clone()).unwrap(),
        summary
    );

    let mut legacy_v1 = rendered.clone();
    legacy_v1["metrics"]["tools"]
        .as_object_mut()
        .unwrap()
        .remove("ordinary");
    let structure = legacy_v1["metrics"]["structure"].as_object_mut().unwrap();
    structure.remove("mutation_turns");
    structure.remove("single_mutation_turns");
    structure.remove("max_mutations_per_turn");
    let legacy_v1 = serde_json::from_value::<RunSummaryV1>(legacy_v1).unwrap();
    assert_eq!(legacy_v1.metrics.tools.as_ref().unwrap().ordinary, None);
    let structure = legacy_v1.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.mutation_turns, None);
    assert_eq!(structure.single_mutation_turns, None);
    assert_eq!(structure.max_mutations_per_turn, None);

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
