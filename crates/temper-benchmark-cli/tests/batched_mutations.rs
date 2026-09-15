//! Explicit multi-file tools count as individual mutation calls and turns.

use std::path::Path;

use serde_json::json;
use temper_benchmark_cli::{AnalyzeOptions, TraceDiagnosticCodeV1, analyze_trace, ingest_trace};
use temper_protocol_activity::{
    AgentActivityEventV1, CapturedContentV1, InlineContentV1, ToolStatusV1,
};

fn inline(text: String) -> CapturedContentV1 {
    CapturedContentV1::Inline(InlineContentV1 {
        text,
        truncated: false,
    })
}

fn trace() -> temper_benchmark_cli::NormalizedTrace {
    let mut trace =
        ingest_trace(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/metrics-events.jsonl"))
            .unwrap();
    for event in &mut trace.events {
        match &mut event.event {
            AgentActivityEventV1::ToolStarted(tool) => match tool.name.as_str() {
                "edit" => {
                    tool.name = "edit_files".into();
                    let files = (0..8)
                        .map(|i| {
                            json!({
                                "path": format!("{i}.rs"),
                                "edits": [{"oldText": "old", "newText": "new"}]
                            })
                        })
                        .collect::<Vec<_>>();
                    tool.arguments = Some(inline(json!({"files": files}).to_string()));
                }
                "write" => {
                    tool.name = "format_rust".into();
                    tool.arguments = Some(inline(
                        json!({
                            "paths": ["one.rs", "two.rs", "three.rs", "four.rs"],
                            "edition": "2021"
                        })
                        .to_string(),
                    ));
                }
                _ => {}
            },
            AgentActivityEventV1::ToolFinished(tool) => match tool.name.as_str() {
                "edit" => tool.name = "edit_files".into(),
                "write" => tool.name = "format_rust".into(),
                _ => {}
            },
            _ => {}
        }
    }
    trace
}

fn options() -> AnalyzeOptions {
    AnalyzeOptions {
        validation_command_prefixes: vec!["cargo test".into()],
        ..AnalyzeOptions::default()
    }
}

#[test]
fn batched_mutations_count_calls_and_preserve_validation_invalidations() {
    let summary = analyze_trace(&trace(), &options());
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.failed_edit_attempts, Some(1));
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.mutation_turns, Some(1));
    assert_eq!(structure.single_mutation_turns, Some(0));
    assert_eq!(structure.max_mutations_per_turn, Some(3));
    assert_eq!(structure.validation_boundaries, Some(3));
    assert_eq!(structure.post_validation_mutations, Some(2));
    assert_eq!(structure.validation_invalidations, Some(2));
    assert_eq!(structure.revalidations, Some(2));
    let tools = summary.metrics.tools.as_ref().unwrap();
    assert_eq!(tools.by_name["edit_files"].calls, 2);
    assert_eq!(tools.by_name["edit_files"].failed, 1);
    assert_eq!(tools.by_name["format_rust"].calls, 2);
}

#[test]
fn batched_mutations_keep_counts_without_arguments_but_require_turn_identity() {
    let mut trace = trace();
    for event in &mut trace.events {
        match &mut event.event {
            AgentActivityEventV1::ToolStarted(tool)
                if matches!(tool.name.as_str(), "edit_files" | "format_rust") =>
            {
                tool.arguments = None
            }
            AgentActivityEventV1::ToolFinished(tool)
                if matches!(tool.name.as_str(), "edit_files" | "format_rust")
                    && tool.status == ToolStatusV1::Succeeded =>
            {
                event.turn = None
            }
            _ => {}
        }
    }
    let summary = analyze_trace(&trace, &options());
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.failed_edit_attempts, Some(1));
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.post_validation_mutations, Some(2));
    assert_eq!(structure.validation_invalidations, Some(2));
    assert_eq!(structure.mutation_turns, None);
    assert_eq!(structure.single_mutation_turns, None);
    assert_eq!(structure.max_mutations_per_turn, None);
    assert!(summary.diagnostics.iter().any(|diagnostic| diagnostic.code
        == TraceDiagnosticCodeV1::StructureEvidenceUnavailable
        && diagnostic.message.contains("lacks turn identity")));
}
