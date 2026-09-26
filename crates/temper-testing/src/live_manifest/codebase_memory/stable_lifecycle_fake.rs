//! Gate basic notes creation on the selected current-checkout source chain.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, Script};
use jig_server::FakeLlm;

use super::{
    MAX_MODEL_MESSAGE_BYTES, ModelObservations, RAW_PROVIDER_FAILURE_NEEDLE, messages_contain,
    stable_lifecycle,
};

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
    let reply = script.next_reply(view);
    if reply.turns.iter().any(|turn| {
        matches!(turn, jig_core::Turn::ToolCall { name, .. } if name == "apply_patch" || name == "submit_for_pr")
    }) {
        assert!(verified_result(view), "stable lifecycle must receive complete graph source before creating notes");
    }
    reply
}

fn record(view: &RequestView, observations: &mut ModelObservations) {
    observations.prompt_guidance_seen |= messages_contain(view, "CODEBASE MEMORY");
    super::legacy_graph_observations::record(view, observations);
    observations.raw_provider_text_seen |= messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE);
    observations.oversized_message_seen |= view
        .messages
        .iter()
        .any(|message| message.role == "tool" && message.content.len() > MAX_MODEL_MESSAGE_BYTES);
}

pub(super) fn verified_result(view: &RequestView) -> bool {
    let mut observations = ModelObservations::default();
    super::legacy_graph_observations::record(view, &mut observations);
    stable_lifecycle::complete_source_observations(&observations)
}
