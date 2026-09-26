//! Gate the basic Jig write on the current checkout's exact search match.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script};
use jig_server::FakeLlm;
use serde_json::Value;

use super::{
    MAX_MODEL_MESSAGE_BYTES, ModelObservations, RAW_PROVIDER_FAILURE_NEEDLE, messages_contain,
    stable_lifecycle,
};

const README: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/codebase-memory-agent/repo/README.md"
));

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
    script: Script,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if !messages_contain(view, "ROLE: engineer") {
            return Reply::text("unexpected stable lifecycle fake-LLM request");
        }
        request_count.fetch_add(1, Ordering::SeqCst);
        record(view, &mut observations.lock().expect("observations lock"));
        reply(view, &script)
    }))
    .map_err(|error| format!("start stable lifecycle Jig fake LLM: {error}"))
}

pub(super) fn reply(view: &RequestView, script: &Script) -> Reply {
    assert!(
        view.prior_tool_results == 0 || verified_result(view),
        "stable lifecycle must receive its verified search source before writing notes"
    );
    script.next_reply(view)
}

fn record(view: &RequestView, observations: &mut ModelObservations) {
    observations.prompt_guidance_seen |= messages_contain(view, "CODEBASE MEMORY");
    let received = verified_result(view);
    observations.memory_result_seen |= received;
    observations.current_root_source_seen |= received;
    observations.current_root_source_results = usize::from(observations.current_root_source_seen);
    observations.raw_provider_text_seen |= messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE);
    observations.oversized_message_seen |= view
        .messages
        .iter()
        .any(|message| message.role == "tool" && message.content.len() > MAX_MODEL_MESSAGE_BYTES);
}

pub(super) fn verified_result(view: &RequestView) -> bool {
    let expected = expected_match();
    view.messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| {
            let content = message
                .content
                .split_once("\n\n[Decision anchor:")
                .map_or(message.content.as_str(), |(result, _)| result);
            serde_json::from_str::<Value>(content).ok()
        })
        .any(|result| {
            result["matches"] == serde_json::json!([expected])
                && result["total"] == 1
                && result["has_more"] == false
        })
}

pub(super) fn expected_match() -> Value {
    let matches = README
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(stable_lifecycle::PATTERN))
        .collect::<Vec<_>>();
    let [(index, content)] = matches.as_slice() else {
        panic!("basic scenario README must contain exactly one search match");
    };
    serde_json::json!({"file_path":"README.md", "line":index + 1,"content":content})
}
