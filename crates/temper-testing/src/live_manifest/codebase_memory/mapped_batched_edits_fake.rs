//! One denied batch, then the identical batch after the companion read.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::{Value, json};
use temper_agent_core::DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE;

use super::{ModelObservations, mapped_graph_consumption_fake, messages_contain};
use crate::live_manifest::batched_edits::{PRIMARY_AFTER, PRIMARY_BEFORE};
use crate::live_manifest::companion_read::{COMPANION_AFTER, COMPANION_BEFORE};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    mapped_graph_consumption_fake::start_with_reply(request_count, observations, reply)
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0..=7 => mapped_graph_consumption_fake::reply(view),
        8 => tool("batch-unread-companion-denied", "edit_files", batch()),
        9 => {
            assert!(messages_contain(
                view,
                DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE
            ));
            tool(
                "read-existing-companion",
                "read",
                json!({"path":"demo/README.md"}),
            )
        }
        10 => tool("batch-read-primary-and-companion", "edit_files", batch()),
        11 => {
            assert!(messages_contain(view, "Edited 2 file(s)"));
            tool(
                "validate-minimal-mapped-repair",
                "bash",
                json!({
                    "command":"cd demo && cargo fmt --check && cargo test --quiet", "timeout":60,
                }),
            )
        }
        12 => tool(
            "submit-mapped-graph-repair",
            "submit_for_pr",
            json!({
                "summary":"Read both existing files before applying their exact replacements in one call.",
            }),
        ),
        13 => Reply::text(
            r##"{"title":"Keep preferred dispatch and its companion documentation","body":"# Implementation report\nRead both existing files before applying their exact replacements in one call. `cargo fmt --check` and `cargo test --quiet` pass.","summary":"Applied one admitted batch after the primary and companion reads."}"##,
        ),
        turn => panic!("unexpected mapped batched-edit model turn {turn}"),
    }
}

fn batch() -> Value {
    json!({"files":[
        {"path":"demo/src/lib.rs","edits":[{"oldText":PRIMARY_BEFORE,"newText":PRIMARY_AFTER}]},
        {"path":"demo/README.md","edits":[{"oldText":COMPANION_BEFORE,"newText":COMPANION_AFTER}]},
    ]})
}

fn tool(id: &str, name: &str, args: Value) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: id.into(),
            name: name.into(),
            args,
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}
