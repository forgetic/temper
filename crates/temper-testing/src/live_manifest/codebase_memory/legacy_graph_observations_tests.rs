use super::*;
use jig_core::{Dialect, ViewMessage};
use serde_json::json;

fn source(kind: usize) -> Value {
    let expected = &SOURCES[kind];
    json!({"qualified_name":expected.symbol,"file_path":expected.path,"source":expected.source})
}

// The model-visible payloads retain no marker or provider binding claim.
fn transcript() -> Vec<Value> {
    vec![
        json!({"results":[{"results":[
            {"qualifiedName":"retry_worker_topic","file_path":"src/lib.rs","is_test":false},
            {"qualifiedName":"alias_retries_keep_the_original_ordered_worker",
                "file_path":"tests/retry_affinity.rs","is_test":true}
        ]}]}),
        json!({"results":[{"qualified_name":"retry_worker_topic","file_path":"src/lib.rs",
            "related_source_references":[{"qualifiedName":"dispatch"}]}]}),
        source(0),
        json!({"function":{"qualified_name":"retry_worker_topic"},"direction":"inbound",
            "complete":true,"callers":[{"qualified_name":"dispatch","file_path":"src/caller.rs"}],
            "related_sources":[{"qualified_name":"dispatch","file_path":"src/caller.rs"}]}),
        source(1),
        source(2),
    ]
}

fn view(results: &[Value]) -> RequestView {
    RequestView::new(
        Dialect::OpenAi,
        None,
        results
            .iter()
            .map(|result| ViewMessage {
                role: "tool".into(),
                content: format!("{result}\n\n[Decision anchor: selected current source]"),
            })
            .collect(),
        1,
    )
}

fn observe(results: &[Value]) -> ModelObservations {
    let mut observations = ModelObservations::default();
    record(&view(results), &mut observations);
    observations
}

#[test]
fn marker_free_model_results_establish_the_six_call_graph_chain() {
    let results = transcript();
    assert_eq!(results.len(), 6);
    let mut observations = observe(&results);
    assert!(observations.memory_result_seen);
    assert!(observations.code_refinement_seen);
    assert!(observations.graph_trace_seen);
    assert!(observations.current_root_source_seen);
    assert_eq!(observations.current_root_source_results, 3);
    record(&view(&results), &mut observations);
    assert_eq!(observations.current_root_source_results, 3);
}

#[test]
fn malformed_payloads_and_markers_do_not_establish_evidence() {
    for content in [
        "{\"results\":[",
        "FAKE_MCP_GRAPH_RESULT FAKE_MCP_CODE_RESULT FAKE_MCP_TRACE_RESULT",
        "{\"binding\":\"current_prepared_checkout\",\"source\":\"unverified\"}",
    ] {
        let mut request = view(&[]);
        request.messages.push(ViewMessage {
            role: "tool".into(),
            content: content.into(),
        });
        let mut observations = ModelObservations::default();
        record(&request, &mut observations);
        assert!(!observations.memory_result_seen);
        assert!(!observations.code_refinement_seen);
        assert!(!observations.graph_trace_seen);
        assert!(!observations.current_root_source_seen);
    }
}

#[test]
fn missing_or_foreign_graph_references_do_not_establish_discovery() {
    for (index, pointer) in [
        (0, "/results/0/results/0/qualifiedName"),
        (0, "/results/0/results/1/file_path"),
        (1, "/results/0/related_source_references/0/qualifiedName"),
        (3, "/callers/0/file_path"),
        (3, "/related_sources/0/qualified_name"),
    ] {
        for wrong in [Value::Null, json!("foreign")] {
            let mut results = transcript();
            *results[index].pointer_mut(pointer).unwrap() = wrong;
            let observations = observe(&results);
            let seen = match index {
                0 => observations.memory_result_seen,
                1 => observations.code_refinement_seen,
                3 => observations.graph_trace_seen,
                _ => unreachable!(),
            };
            assert!(!seen, "{index}: {pointer}");
        }
    }
}

#[test]
fn claimed_binding_cannot_hide_missing_or_foreign_source_evidence() {
    for index in [2, 4, 5] {
        for field in ["qualified_name", "file_path", "source"] {
            for wrong in [Value::Null, json!("foreign"), json!("")] {
                let mut results = transcript();
                results[index][field] = wrong;
                results[index]["binding"] = json!("current_prepared_checkout");
                assert_eq!(
                    observe(&results).current_root_source_results,
                    2,
                    "{index}: {field}"
                );
            }
        }
    }
}

#[test]
fn source_requires_prior_selection_and_distinct_roles() {
    let mut results = transcript();
    results.swap(0, 2);
    assert_eq!(observe(&results).current_root_source_results, 2);
    results = transcript();
    results[4] = source(0);
    results[5] = source(0);
    assert_eq!(observe(&results).current_root_source_results, 1);
    results = transcript();
    results.remove(0);
    results.remove(2);
    assert_eq!(observe(&results).current_root_source_results, 0);
}

#[test]
fn ordinary_messages_cannot_supply_provider_evidence() {
    for role in ["user", "assistant", "system"] {
        let mut request = view(&transcript());
        for message in &mut request.messages {
            message.role = role.into();
        }
        let mut observations = ModelObservations::default();
        record(&request, &mut observations);
        assert!(!observations.memory_result_seen);
        assert!(!observations.code_refinement_seen);
        assert!(!observations.graph_trace_seen);
        assert!(!observations.current_root_source_seen);
    }
}
