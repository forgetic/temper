//! Jig-backed end-to-end test for the native sans-IO sub-agent loop.
//!
//! Drives a real [`smith_agent::run_sub_agent`] (pure `AgentMachine` + asupersync
//! `AgentShell` + pi-SDK provider + pi-SDK tools) against a local scripted
//! `jig_server::FakeLlm`, entirely in-process on the asupersync runtime. The
//! fake instructs the agent to call the `write` tool to create a file, then to
//! finish; the test asserts the file landed, the loop ran a tool round, and the
//! run completed cleanly. This is the native-loop analog of
//! `smith-temper-agent`'s `jig_coding_agent.rs` (which drives pi's own loop).
//!
//! Hermetic and fast — no live provider — so it runs by default.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use pi::provider::StreamOptions;
use pi::sdk::{create_read_tool, create_write_tool};
use pi::tools::ToolRegistry;
use smith_agent::{AgentStop, SubAgent, run_sub_agent};
use smith_temper_agent::ProviderConfig;

#[test]
fn sub_agent_runs_a_tool_loop_and_completes() {
    let observed_continuation = Arc::new(AtomicUsize::new(0));
    let fake = sub_agent_fake(Arc::clone(&observed_continuation));

    let checkout = TempCheckout::new("sub-agent-tool-loop");

    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-sub-agent-tool-loop",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
    .build_provider()
    .expect("build jig provider");

    let tools = ToolRegistry::from_tools(vec![
        create_read_tool(checkout.path()),
        create_write_tool(checkout.path()),
    ]);

    let outcome = smith_io_engine::block_on(async move {
        run_sub_agent(SubAgent {
            system_prompt: Some(
                "You are a sub-agent. Use the write tool to create the requested file."
                    .to_string(),
            ),
            user_message: "Create NOTES.md whose first line is exactly `project notes`."
                .to_string(),
            tools,
            max_iterations: 6,
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        })
        .await
    })
    .expect("sub-agent runs");

    assert_eq!(outcome.stop, AgentStop::Completed, "run should complete cleanly");

    // The write tool actually created the file in the checkout.
    let notes = fs::read_to_string(checkout.path().join("NOTES.md")).expect("NOTES.md was written");
    assert_eq!(
        notes.lines().next(),
        Some("project notes"),
        "NOTES.md first line must match the requested content"
    );

    // The loop did a tool round (more than one model turn) and the fake saw the
    // tool-result continuation.
    assert!(
        fake.requests().len() > 1,
        "expected a tool loop, got a single model turn"
    );
    assert!(
        observed_continuation.load(Ordering::SeqCst) >= 1,
        "fake provider did not observe a tool-result continuation turn"
    );

    // The final conversation ends with the model's terminal text message.
    assert!(
        outcome
            .final_message
            .content
            .iter()
            .any(|block| matches!(block, pi::model::ContentBlock::Text(_))),
        "final message should carry the model's closing text"
    );
}

#[test]
fn sub_agent_reports_budget_exhaustion_when_model_loops_forever() {
    // The fake always asks for another tool call; the agent must stop at the
    // iteration budget rather than loop unboundedly.
    let fake = FakeLlm::start(Script::rule(|_view| Reply {
        turns: vec![Turn::ToolCall {
            id: "call_again".to_string(),
            name: "read".to_string(),
            args: serde_json::json!({ "path": "NOTES.md" }),
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }))
    .expect("start fake LLM");

    let checkout = TempCheckout::new("sub-agent-budget");
    fs::write(checkout.path().join("NOTES.md"), "seed\n").expect("seed file");

    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-sub-agent-budget",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
    .build_provider()
    .expect("build jig provider");

    let tools = ToolRegistry::from_tools(vec![create_read_tool(checkout.path())]);

    let outcome = smith_io_engine::block_on(async move {
        run_sub_agent(SubAgent {
            system_prompt: None,
            user_message: "read forever".to_string(),
            tools,
            max_iterations: 3,
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        })
        .await
    })
    .expect("sub-agent runs");

    assert_eq!(outcome.stop, AgentStop::BudgetExhausted);
}

#[test]
fn sub_agent_forwards_live_events_to_the_sink() {
    use std::sync::Mutex;
    use smith_agent::{AgentEvent, EventSink, StreamDelta, run_sub_agent_with_events};

    // A sink that records every event it sees.
    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<AgentEvent>>,
    }
    impl EventSink for Recorder {
        fn emit(&self, event: AgentEvent) {
            self.events.lock().expect("events lock").push(event);
        }
    }

    let observed = Arc::new(AtomicUsize::new(0));
    let fake = sub_agent_fake(Arc::clone(&observed));
    let checkout = TempCheckout::new("sub-agent-events");

    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-sub-agent-events",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
    .build_provider()
    .expect("build jig provider");

    let tools = ToolRegistry::from_tools(vec![
        create_read_tool(checkout.path()),
        create_write_tool(checkout.path()),
    ]);

    let recorder = Arc::new(Recorder::default());
    let recorder_for_run = Arc::clone(&recorder);
    let outcome = smith_io_engine::block_on(async move {
        run_sub_agent_with_events(
            SubAgent {
                system_prompt: Some("Use the write tool.".to_string()),
                user_message: "Create NOTES.md.".to_string(),
                tools,
                max_iterations: 6,
                provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
            recorder_for_run,
        )
        .await
    })
    .expect("sub-agent runs");
    assert_eq!(outcome.stop, AgentStop::Completed);

    let events = recorder.events.lock().expect("events lock");
    // Lifecycle events present.
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::TurnStart { .. })),
        "expected a TurnStart event"
    );
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolStart { .. })),
        "expected a ToolStart event (the write tool)"
    );
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
        "expected an AgentEnd event"
    );
    // Live streaming deltas were forwarded by the shell. The jig fake streams a
    // tool call on the first turn (ToolCall delta) and text on the second
    // (Text delta), so at least one StreamDelta must appear.
    let stream_deltas = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::StreamDelta(_)))
        .count();
    assert!(
        stream_deltas > 0,
        "expected live StreamDelta events from the shell; got events: {events:?}"
    );
    // And specifically the tool-call delta for `write`.
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::StreamDelta(StreamDelta::ToolCall { name, .. }) if name == "write"
        )),
        "expected a streamed tool-call delta for `write`"
    );
}

#[test]
fn sub_agent_can_be_aborted_mid_run() {
    use smith_agent::{NullEventSink, run_sub_agent_controllable};

    // A fake that loops forever (always asks for another tool call), so the run
    // only ends when aborted.
    let fake = FakeLlm::start(Script::rule(|_view| Reply {
        turns: vec![Turn::ToolCall {
            id: "loop".to_string(),
            name: "read".to_string(),
            args: serde_json::json!({ "path": "X.md" }),
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }))
    .expect("start fake");

    let checkout = TempCheckout::new("sub-agent-abort");
    fs::write(checkout.path().join("X.md"), "x\n").expect("seed");

    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-abort",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
    .build_provider()
    .expect("build provider");

    let tools = ToolRegistry::from_tools(vec![create_read_tool(checkout.path())]);

    let outcome = smith_io_engine::block_on(async move {
        // A high iteration budget so the run would otherwise spin for a long
        // time; abort is what ends it.
        let (control, run) = run_sub_agent_controllable(
            SubAgent {
                system_prompt: None,
                user_message: "loop".to_string(),
                tools,
                max_iterations: 100,
                provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
            Arc::new(NullEventSink),
        )
        .expect("build controllable run");

        // Abort from a sibling task after letting the run get going.
        let handle = asupersync::runtime::Runtime::current_handle().expect("handle");
        handle.spawn_with_cx(move |cx| async move {
            // Let a couple of turns happen first (virtual time).
            asupersync::time::sleep(
                smith_io_engine::timer_now(&cx),
                std::time::Duration::from_millis(50),
            )
            .await;
            control.abort();
        });

        run.await
    })
    .expect("run resolves");

    assert_eq!(outcome.stop, AgentStop::Aborted, "abort should stop the run");
    // It did not run anywhere near the 100-iteration budget.
    assert!(
        fake.requests().len() < 100,
        "abort should stop the loop well before the budget"
    );
}

#[test]
fn sub_agent_steering_reaches_the_model() {
    use smith_agent::{NullEventSink, run_sub_agent_controllable};
    use std::sync::Mutex;

    // The fake records the user-message texts it has seen so we can prove the
    // steered message reached the model's context. It keeps asking for a tool on
    // the first turn, then completes — but if it ever sees the steer text it
    // completes immediately citing it.
    let seen_steer = Arc::new(Mutex::new(false));
    let seen_steer_in = Arc::clone(&seen_steer);
    let fake = FakeLlm::start(Script::rule(move |view| {
        // jig's view exposes the conversation; if any user turn carries our
        // steer marker, acknowledge it.
        let steered = view
            .messages
            .iter()
            .any(|m| m.content.contains("STEER-MARKER"));
        if steered {
            *seen_steer_in.lock().expect("lock") = true;
            return Reply::text("Acknowledged steering: STEER-MARKER seen.");
        }
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "t".to_string(),
                    name: "read".to_string(),
                    args: serde_json::json!({ "path": "X.md" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            Reply::text("done without steering")
        }
    }))
    .expect("start fake");

    let checkout = TempCheckout::new("sub-agent-steer");
    fs::write(checkout.path().join("X.md"), "x\n").expect("seed");

    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-steer",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
    .build_provider()
    .expect("build provider");

    let tools = ToolRegistry::from_tools(vec![create_read_tool(checkout.path())]);

    let outcome = smith_io_engine::block_on(async move {
        let (control, run) = run_sub_agent_controllable(
            SubAgent {
                system_prompt: None,
                user_message: "do a thing".to_string(),
                tools,
                max_iterations: 10,
                provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
            Arc::new(NullEventSink),
        )
        .expect("build controllable run");

        // Queue the steering immediately, before driving: the machine injects
        // it at the first turn boundary (after the first tool round), so the
        // model's second-turn context carries the marker. (Wall-clock-timed
        // injection is unreliable under virtual time, since a non-sleeping run
        // can finish before a timer fires; queueing up front is deterministic.)
        control.steer_text("STEER-MARKER please stop now");

        run.await
    })
    .expect("run resolves");

    assert_eq!(outcome.stop, AgentStop::Completed);
    assert!(
        *seen_steer.lock().expect("lock"),
        "the steered message should have reached the model's context"
    );
}

fn sub_agent_fake(observed_continuation: Arc<AtomicUsize>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_notes".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "NOTES.md",
                        "content": "project notes\n"
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            observed_continuation.fetch_add(1, Ordering::SeqCst);
            Reply::text("Created NOTES.md with project notes.")
        }
    }))
    .expect("start fake LLM")
}

/// A throwaway checkout directory removed on drop.
struct TempCheckout {
    path: PathBuf,
}

impl TempCheckout {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("smith-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create checkout dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
