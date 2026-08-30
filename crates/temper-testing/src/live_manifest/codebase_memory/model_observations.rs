//! Shared privacy-safe observations made from model-visible fixture messages.

#[derive(Default)]
pub(super) struct ModelObservations {
    pub(super) prompt_guidance_seen: bool,
    pub(super) memory_result_seen: bool,
    pub(super) current_root_source_seen: bool,
    pub(super) code_refinement_seen: bool,
    pub(super) graph_trace_seen: bool,
    pub(super) current_root_source_results: usize,
    pub(super) safe_failure_seen: bool,
    pub(super) raw_provider_text_seen: bool,
    pub(super) bounded_graph_result_seen: bool,
    pub(super) oversized_message_seen: bool,
}
