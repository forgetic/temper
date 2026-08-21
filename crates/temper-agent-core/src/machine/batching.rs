//! Effect-compatible tool batching: the pure policy that decides which tool
//! calls may run concurrently and where a serialized barrier falls.
//!
//! This mirrors pi's `plan_tool_effect_batches`: tool calls are never
//! reordered; adjacent calls whose declared [`ToolEffects`] are mutually
//! parallel-safe (read-only) are grouped into one concurrent batch, and a
//! write/network/process tool — or an unknown tool, fail-closed — starts a new
//! serialized batch. The logic is a pure function of the calls and the static
//! effect declarations, so it lives here, apart from the loop that drives it.

use std::collections::{BTreeMap, VecDeque};

use tongs::model::ToolCall;
use tongs::tools::ToolEffects;

/// One tool call in flight within a batch, paired with its result once the
/// shell reports it finished.
pub(super) struct PendingTool {
    pub(super) call: ToolCall,
    pub(super) output: Option<tongs::tools::ToolOutput>,
    pub(super) failure: Option<super::tool_failure::ToolFailureDiagnostic>,
}

/// The effect declaration for a tool name, defaulting to write (serialize) for
/// unknown tools — fail-closed, matching pi.
pub(super) fn effects_for(effects: &BTreeMap<String, ToolEffects>, name: &str) -> ToolEffects {
    effects
        .get(name)
        .copied()
        .unwrap_or_else(ToolEffects::write)
}

/// Partition tool calls into contiguous effect-compatible batches (front of the
/// returned deque first). Never reorders calls, groups adjacent mutually
/// parallel-safe effects, and breaks at the first incompatible effect.
pub(super) fn plan_batches(
    effects: &BTreeMap<String, ToolEffects>,
    calls: &[ToolCall],
) -> VecDeque<Vec<PendingTool>> {
    let mut batches = VecDeque::new();
    let mut current: Vec<PendingTool> = Vec::new();
    let mut active: Option<ToolEffects> = None;
    for call in calls {
        let call_effects = effects_for(effects, &call.name);
        let compatible = match active {
            Some(active_effects) => active_effects.compatible_with(call_effects),
            None => true,
        };
        if compatible && !current.is_empty() {
            active = Some(
                active
                    .expect("active set when current non-empty")
                    .union(call_effects),
            );
            current.push(PendingTool {
                call: call.clone(),
                output: None,
                failure: None,
            });
        } else {
            if !current.is_empty() {
                batches.push_back(std::mem::take(&mut current));
            }
            active = Some(call_effects);
            current.push(PendingTool {
                call: call.clone(),
                output: None,
                failure: None,
            });
        }
    }
    if !current.is_empty() {
        batches.push_back(current);
    }
    batches
}
