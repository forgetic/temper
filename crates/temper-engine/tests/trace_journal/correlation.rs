use super::{RUN_ID, batch, binding, event, finished, open_journal, policy, started};
use temper_protocol_activity::{
    AgentActivityEventV1, CaptureModeV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
    GraphCorrelationV1, ToolFinishedV1, ToolStatusV1,
};

#[test]
fn journal_sanitizes_invalid_graph_correlation_without_retaining_raw_input() {
    const SECRET: &str = "Authorization: Bearer JOURNAL-GRAPH-CORRELATION-SECRET";
    let temporary = tempfile::tempdir().expect("tempdir");
    let policy = policy(CaptureModeV1::Metadata);
    let journal = open_journal(&temporary, &policy);
    let binding = binding(&policy);
    let mut malformed = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        "safe declaration",
    )
    .expect("valid correlation before a forged mutation");
    malformed.target_digest = SECRET.to_string();
    let valid = GraphCorrelationV1::new(
        GraphCorrelationToolV1::SearchCode,
        GraphCorrelationTargetKindV1::Pattern,
        "ToolFinishedV1",
    )
    .expect("valid correlation");
    let invalid_tool = event(
        2,
        20,
        &binding,
        AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
            call_id: "graph-invalid".to_string(),
            name: "codebase_memory_search_graph".to_string(),
            status: ToolStatusV1::Succeeded,
            duration_ms: 4,
            result: None,
            failure: None,
            codebase_memory_timing: None,
            graph_correlation: Some(malformed),
            decision_anchor_lineage: None,
        }),
    );
    let valid_tool = event(
        3,
        30,
        &binding,
        AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
            call_id: "graph-valid".to_string(),
            name: "codebase_memory_search_code".to_string(),
            status: ToolStatusV1::Succeeded,
            duration_ms: 5,
            result: None,
            failure: None,
            codebase_memory_timing: None,
            graph_correlation: Some(valid.clone()),
            decision_anchor_lineage: None,
        }),
    );

    journal
        .ingest(
            &binding,
            &batch(vec![
                started(1, &binding),
                invalid_tool,
                valid_tool,
                finished(4, &binding),
            ]),
        )
        .expect("journal sanitizes before validating a direct batch");
    let events = journal.events(RUN_ID).expect("read durable events");
    let AgentActivityEventV1::ToolFinished(invalid) = &events[1].event else {
        panic!("invalid graph event is durable without a correlation");
    };
    assert_eq!(invalid.graph_correlation, None);
    let AgentActivityEventV1::ToolFinished(preserved) = &events[2].event else {
        panic!("valid graph event is durable");
    };
    assert_eq!(preserved.graph_correlation.as_ref(), Some(&valid));
    let durable = std::fs::read_to_string(journal.run_directory(RUN_ID).join("events.jsonl"))
        .expect("read durable journal");
    assert!(!durable.contains(SECRET));
}
