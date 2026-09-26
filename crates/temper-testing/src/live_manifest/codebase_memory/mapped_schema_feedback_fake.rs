//! Repair a malformed batch using safe feedback before ordinary admission.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use jig_core::{RecordedRequest, Reply, RequestView, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::json;

use super::{ModelObservations, mapped_batched_edits_fake, mapped_graph_consumption_fake};

const SAFE_FEEDBACK: &str = "Tool edit_files: required field $.files[].edits[].oldText is missing.";
const UNKNOWN_KEY: &str = "SCHEMA_FIXTURE_UNKNOWN_KEY_1322";
const REPLACEMENT_VALUE: &str = "SCHEMA_FIXTURE_REPLACEMENT_VALUE_1322";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    mapped_graph_consumption_fake::start_with_reply(request_count, observations, reply)
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0..=7 => mapped_graph_consumption_fake::reply(view),
        8 => Reply {
            turns: vec![Turn::ToolCall {
                id: "batch-missing-old-text".into(),
                name: "edit_files".into(),
                args: json!({"files":[{"path":"demo/src/lib.rs","edits":[{
                    "newText":REPLACEMENT_VALUE, (UNKNOWN_KEY):true,
                }]}]}),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        count => {
            if count == 9 {
                assert!(super::messages_contain(view, SAFE_FEEDBACK));
            }
            mapped_batched_edits_fake::reply_at(view, count - 1)
        }
    }
}

/// Inspect recorded wire bodies as well as projected message content: Jig's
/// normalized view omits some assistant tool-call arguments.
pub(super) fn validate_requests(requests: &[RecordedRequest]) -> Result<(), String> {
    let next = requests
        .iter()
        .find(|request| {
            request
                .view
                .as_ref()
                .is_some_and(|view| view.prior_tool_results == 9)
        })
        .ok_or("schema feedback fixture did not observe the immediate retry request")?;
    if !next.body_str().contains(SAFE_FEEDBACK) {
        return Err("schema feedback fixture did not receive its safe missing-field hint".into());
    }
    for request in requests {
        let body = request.body_str();
        if body.contains(UNKNOWN_KEY) || body.contains(REPLACEMENT_VALUE) {
            return Err("schema feedback fixture leaked caller-controlled sentinel bytes".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mapped_schema_feedback_tests.rs"]
mod tests;
