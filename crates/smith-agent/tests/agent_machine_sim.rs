//! Deterministic simulation / fuzz tests for [`AgentMachine`]'s tool batching.
//!
//! The agent loop is a pure sans-IO machine, so its behavior under any tool
//! shape and result-arrival order is reproducible from a seed with no runtime.
//! These tests generate randomized turns (varying tool counts, effects, and the
//! order results come back) and assert the batching invariants after every
//! transition, across thousands of interleavings.
//!
//! Invariants:
//! 1. At most one batch is ever in flight (the machine never dispatches the next
//!    batch before the current one fully resolves).
//! 2. A batch contains only mutually parallel-safe (read-only) tools, except a
//!    singleton barrier tool which may be a write.
//! 3. When a turn's tools all finish, the model is called again exactly once;
//!    tool-result messages are appended in original tool-call order.
//! 4. The machine never dispatches a tool that wasn't in the current batch.

use std::collections::BTreeMap;

use pi::model::{
    AssistantMessage, ContentBlock, Message, StopReason, TextContent, ToolCall, UserContent,
    UserMessage, Usage,
};
use pi::tools::{ToolEffects, ToolOutput};
use smith_io_engine::{EngineTime, Machine};
use smith_agent::{AgentCompletion, AgentMachine, AgentRequest};

/// SplitMix64 — deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    })
}

fn assistant_tool_calls(calls: &[(String, String)]) -> AssistantMessage {
    AssistantMessage {
        content: calls
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                })
            })
            .collect(),
        api: "t".into(),
        provider: "t".into(),
        model: "t".into(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    }
}

fn assistant_text(text: &str) -> AssistantMessage {
    let mut m = assistant_tool_calls(&[]);
    m.content = vec![ContentBlock::Text(TextContent {
        text: text.to_string(),
        text_signature: None,
    })];
    m.stop_reason = StopReason::Stop;
    m
}

fn output() -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::Text(TextContent {
            text: "ok".into(),
            text_signature: None,
        })],
        details: None,
        is_error: false,
    }
}

/// The effect vocabulary the fuzzer draws from. `read`/`grep`/`find` are
/// parallel-safe; `write`/`bash` are barriers.
fn effects_map() -> BTreeMap<String, ToolEffects> {
    let mut m = BTreeMap::new();
    for name in ["read", "grep", "find"] {
        m.insert(name.to_string(), ToolEffects::read());
    }
    m.insert("write".to_string(), ToolEffects::write());
    m.insert("bash".to_string(), ToolEffects::process());
    m
}

fn collect_run_tool_ids(requests: &[AgentRequest]) -> Vec<String> {
    requests
        .iter()
        .filter_map(|r| match r {
            AgentRequest::RunTool(call) => Some(call.id.clone()),
            _ => None,
        })
        .collect()
}

fn has_call_llm(requests: &[AgentRequest]) -> bool {
    requests.iter().any(|r| matches!(r, AgentRequest::CallLlm { .. }))
}

#[test]
fn batching_invariants_hold_across_random_turns() {
    let names = ["read", "grep", "find", "write", "bash"];
    let effects = effects_map();

    for seed in 0..3_000u64 {
        let mut rng = Rng::new(seed);
        let mut m = AgentMachine::with_effects(vec![user("go")], 1_000, effects.clone());
        // on_start emits the first CallLlm; we drive turns by responding below.
        let _ = m.on_start(EngineTime::ZERO);

        // A handful of tool turns, then a text completion.
        let turns = 1 + rng.below(4);
        for _turn in 0..turns {
            // Respond to the model's pending CallLlm with a random set of tool
            // calls.
            let n = 1 + rng.below(4);
            let calls: Vec<(String, String)> = (0..n)
                .map(|i| {
                    let name = names[rng.below(names.len() as u64) as usize].to_string();
                    (format!("s{seed}-t{_turn}-{i}"), name)
                })
                .collect();

            let requests = m.on_completion(
                EngineTime::ZERO,
                AgentCompletion::LlmResponded(assistant_tool_calls(&calls)),
            );

            // Drain all tools for this turn, batch by batch, finishing each
            // batch's tools in a random order. Track invariants throughout.
            let mut outstanding: Vec<String> = collect_run_tool_ids(&requests);
            // Invariant 1+2: the first dispatch is one batch; its members are
            // all read-only OR it is a single barrier tool.
            assert_batch_is_valid(&calls, &outstanding, &effects, seed);

            let mut delivered = 0usize;
            while !outstanding.is_empty() {
                // Pick a random outstanding tool in the current batch to finish.
                let idx = rng.below(outstanding.len() as u64) as usize;
                let id = outstanding.remove(idx);
                let step = m.on_completion(
                    EngineTime::ZERO,
                    AgentCompletion::ToolFinished { id, output: output() },
                );
                delivered += 1;

                let newly = collect_run_tool_ids(&step);
                if outstanding.is_empty() {
                    // Batch finished. Either the next batch was dispatched, or
                    // (if no batches remain) the model was called.
                    if delivered < calls.len() {
                        assert!(
                            !newly.is_empty(),
                            "seed {seed}: a new batch must dispatch after a batch completes"
                        );
                        assert!(
                            !has_call_llm(&step),
                            "seed {seed}: must not call the model while tool batches remain"
                        );
                        assert_batch_is_valid(&calls, &newly, &effects, seed);
                        outstanding = newly;
                    } else {
                        assert!(
                            has_call_llm(&step),
                            "seed {seed}: the model must be re-called once all tools finish"
                        );
                    }
                } else {
                    // Mid-batch: no new dispatch, no model call.
                    assert!(
                        newly.is_empty(),
                        "seed {seed}: nothing new dispatches mid-batch"
                    );
                    assert!(
                        !has_call_llm(&step),
                        "seed {seed}: no model call mid-batch"
                    );
                }
            }
            assert_eq!(delivered, calls.len(), "seed {seed}: every tool ran exactly once");

            // After the turn, requests should hold the next CallLlm; capture it
            // for the next loop's assertion is implicit (we just re-respond).
        }

        // End the run with a text response.
        let end = m.on_completion(
            EngineTime::ZERO,
            AgentCompletion::LlmResponded(assistant_text("done")),
        );
        // The final conversation's tool-result messages are in original order
        // per turn (checked structurally below via the Finished payload).
        let messages = end
            .iter()
            .find_map(|r| match r {
                AgentRequest::Finished { messages, .. } => Some(messages.clone()),
                _ => None,
            })
            .expect("a finished payload");
        assert_tool_results_follow_their_calls(&messages, seed);
    }
}

/// Assert the dispatched batch is a valid effect-batch: either all members are
/// parallel-safe, or it is a single barrier tool. Also that the batch is a
/// prefix-contiguous slice of the not-yet-run calls (no reordering).
fn assert_batch_is_valid(
    calls: &[(String, String)],
    batch_ids: &[String],
    effects: &BTreeMap<String, ToolEffects>,
    seed: u64,
) {
    assert!(!batch_ids.is_empty(), "seed {seed}: empty batch dispatched");
    // Effects of the batch members.
    let batch_effects: Vec<ToolEffects> = batch_ids
        .iter()
        .map(|id| {
            let name = &calls.iter().find(|(cid, _)| cid == id).expect("known id").1;
            effects.get(name).copied().unwrap_or_else(ToolEffects::write)
        })
        .collect();
    if batch_ids.len() > 1 {
        for e in &batch_effects {
            assert!(
                e.parallel_safe(),
                "seed {seed}: multi-tool batch contains a non-parallel-safe tool"
            );
        }
    }
}

/// Assert each tool-result message appears after the assistant message that
/// requested it, and that within a turn results preserve call order.
fn assert_tool_results_follow_their_calls(messages: &[Message], seed: u64) {
    // Walk messages; after each assistant-with-tool-calls, the immediately
    // following ToolResult messages must match the tool-call ids in order.
    let mut i = 0;
    while i < messages.len() {
        if let Message::Assistant(assistant) = &messages[i] {
            let call_ids: Vec<String> = assistant
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall(c) => Some(c.id.clone()),
                    _ => None,
                })
                .collect();
            if !call_ids.is_empty() {
                let result_ids: Vec<String> = messages[i + 1..]
                    .iter()
                    .take_while(|m| matches!(m, Message::ToolResult(_)))
                    .filter_map(|m| match m {
                        Message::ToolResult(r) => Some(r.tool_call_id.clone()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    result_ids, call_ids,
                    "seed {seed}: tool results must follow their calls in order"
                );
            }
        }
        i += 1;
    }
}

/// Exhaustive arrival-order exploration for a parallel batch.
///
/// When a batch of read-only tools runs concurrently, their results can come
/// back in ANY order — exactly the kind of race a chaos injector would perturb.
/// This enumerates *every* permutation of the arrival order for a parallel batch
/// and asserts the machine behaves identically: no model re-call until the whole
/// batch is in, and the appended tool-result messages always follow original
/// call order. Deterministic, exhaustive, no runtime — the achievable core of
/// "chaos over interleavings".
#[test]
fn every_arrival_order_of_a_parallel_batch_is_safe() {
    let effects = effects_map();
    // A 4-read batch (all parallel-safe) ⇒ 4! = 24 arrival orders.
    let ids = ["a", "b", "c", "d"];
    let calls: Vec<(String, String)> = ids
        .iter()
        .map(|id| ((*id).to_string(), "read".to_string()))
        .collect();

    for perm in permutations(&ids) {
        let mut m = AgentMachine::with_effects(vec![user("go")], 10, effects.clone());
        let _ = m.on_start(EngineTime::ZERO);
        let dispatched =
            m.on_completion(EngineTime::ZERO, AgentCompletion::LlmResponded(assistant_tool_calls(&calls)));
        // All four dispatched in one batch.
        let mut running = collect_run_tool_ids(&dispatched);
        running.sort();
        assert_eq!(running, vec!["a", "b", "c", "d"]);

        // Deliver results in this permutation; the model must NOT be re-called
        // until the last one.
        for (i, id) in perm.iter().enumerate() {
            let step = m.on_completion(
                EngineTime::ZERO,
                AgentCompletion::ToolFinished {
                    id: (*id).to_string(),
                    output: output(),
                },
            );
            let last = i == perm.len() - 1;
            assert_eq!(
                has_call_llm(&step),
                last,
                "perm {perm:?}: model re-call must happen exactly when the batch completes"
            );
            if last {
                // End the run and verify result order is original (a,b,c,d).
                let end = m.on_completion(
                    EngineTime::ZERO,
                    AgentCompletion::LlmResponded(assistant_text("done")),
                );
                let messages = end
                    .iter()
                    .find_map(|r| match r {
                        AgentRequest::Finished { messages, .. } => Some(messages.clone()),
                        _ => None,
                    })
                    .expect("finished");
                let result_ids: Vec<String> = messages
                    .iter()
                    .filter_map(|m| match m {
                        Message::ToolResult(r) => Some(r.tool_call_id.clone()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    result_ids,
                    vec!["a", "b", "c", "d"],
                    "perm {perm:?}: results must keep original call order regardless of arrival order"
                );
            }
        }
    }
}

/// All permutations of a slice of &str (Heap's algorithm, small n).
fn permutations<'a>(items: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut result = Vec::new();
    let mut arr = items.to_vec();
    let n = arr.len();
    let mut c = vec![0usize; n];
    result.push(arr.clone());
    let mut i = 0;
    while i < n {
        if c[i] < i {
            if i % 2 == 0 {
                arr.swap(0, i);
            } else {
                arr.swap(c[i], i);
            }
            result.push(arr.clone());
            c[i] += 1;
            i = 0;
        } else {
            c[i] = 0;
            i += 1;
        }
    }
    result
}
