//! Jig-backed end-to-end test for the native sans-IO sub-agent loop.
//!
//! Drives a real [`temper_agent_core::run_sub_agent`] (pure `AgentMachine` + asupersync
//! `AgentShell` + pi-SDK provider + pi-SDK tools) against a local scripted
//! `jig_server::FakeLlm`, entirely in-process on the asupersync runtime. The
//! fake instructs the agent to call the `write` tool to create a file, then to
//! finish; the test asserts the file landed, the loop ran a tool round, and the
//! run completed cleanly. This is the native-loop analog of
//! `anvil-temper-agent`'s `jig_coding_agent.rs` (which drives pi's own loop).
//!
//! Hermetic and fast — no live provider — so it runs by default.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::ProviderConfig;
use temper_agent_core::{AgentStop, SubAgent, TurnHook, run_sub_agent, run_sub_agent_with_hook};
use tongs::provider::StreamOptions;
use tongs::tools::ToolRegistry;
use tongs::tools::{create_read_tool, create_write_tool, tool_to_definition};

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

    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_sub_agent(
            handle,
            SubAgent {
                system_prompt: Some(
                    "You are a sub-agent. Use the write tool to create the requested file."
                        .to_string(),
                ),
                user_message: "Create NOTES.md whose first line is exactly `project notes`."
                    .to_string(),
                tools,
                max_iterations: 6,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
                provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
        )
        .await
    })
    .expect("sub-agent runs");

    assert_eq!(
        outcome.stop,
        AgentStop::Completed,
        "run should complete cleanly"
    );

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
            .any(|block| matches!(block, tongs::model::ContentBlock::Text(_))),
        "final message should carry the model's closing text"
    );
}

/// Records the `turn` of every hook invocation.
struct CountingHook {
    turns: std::sync::Mutex<Vec<usize>>,
}

#[async_trait::async_trait]
impl TurnHook for CountingHook {
    async fn before_model_call(&self, turn: usize) {
        self.turns.lock().expect("turns lock").push(turn);
    }
}

#[test]
fn turn_hook_runs_before_every_model_call() {
    let observed_continuation = Arc::new(AtomicUsize::new(0));
    let fake = sub_agent_fake(Arc::clone(&observed_continuation));
    let checkout = TempCheckout::new("sub-agent-turn-hook");

    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-sub-agent-turn-hook",
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
    let hook = Arc::new(CountingHook {
        turns: std::sync::Mutex::new(Vec::new()),
    });

    let hook_for_run = Arc::clone(&hook);
    let outcome = temper_agent_io::block_on_with(move |_cx, handle| {
        let hook = hook_for_run;
        async move {
            run_sub_agent_with_hook(
                handle,
                SubAgent {
                    system_prompt: Some(
                        "You are a sub-agent. Use the write tool to create the requested file."
                            .to_string(),
                    ),
                    user_message: "Create NOTES.md whose first line is exactly `project notes`."
                        .to_string(),
                    tools,
                    max_iterations: 6,
                    operation_limits: temper_agent_core::AgentOperationLimits::default(),
                    provider,
                    stream_options: StreamOptions {
                        api_key: Some("sk-jig-test".to_string()),
                        ..StreamOptions::default()
                    },
                },
                hook,
            )
            .await
        }
    })
    .expect("sub-agent runs");

    assert_eq!(outcome.stop, AgentStop::Completed);
    let turns = hook.turns.lock().expect("turns lock").clone();
    let model_calls = fake.requests().len();
    assert_eq!(
        turns.len(),
        model_calls,
        "the hook must run once per model call (saw {turns:?})"
    );
    assert_eq!(
        turns,
        (0..model_calls).collect::<Vec<_>>(),
        "turn numbers are zero-based and monotonic"
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

    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_sub_agent(
            handle,
            SubAgent {
                system_prompt: None,
                user_message: "read forever".to_string(),
                tools,
                max_iterations: 3,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
                provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
        )
        .await
    })
    .expect("sub-agent runs");

    assert_eq!(outcome.stop, AgentStop::BudgetExhausted);
}

#[test]
fn sub_agent_forwards_live_events_to_the_sink() {
    use std::sync::Mutex;
    use temper_agent_core::{AgentEvent, EventSink, StreamDelta, run_sub_agent_with_events};

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
    let expected_tools = tools
        .tools()
        .iter()
        .map(|tool| tool_to_definition(tool.as_ref()))
        .collect::<Vec<_>>();
    let expected_system = "Use the write tool.";
    let expected_user = "Create NOTES.md.";

    let recorder = Arc::new(Recorder::default());
    let recorder_for_run = Arc::clone(&recorder);
    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_sub_agent_with_events(
            handle,
            SubAgent {
                system_prompt: Some(expected_system.to_string()),
                user_message: expected_user.to_string(),
                tools,
                max_iterations: 6,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
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
    let prompt_positions = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, AgentEvent::PromptPrepared { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(prompt_positions.len(), 1, "one prompt per invocation");
    let first_turn = events
        .iter()
        .position(|event| matches!(event, AgentEvent::TurnStart { .. }))
        .expect("turn start");
    assert!(
        prompt_positions[0] < first_turn,
        "prompt precedes first turn"
    );
    let AgentEvent::PromptPrepared {
        system_prompt,
        initial_user_message,
        tools,
    } = &events[prompt_positions[0]]
    else {
        unreachable!();
    };
    assert_eq!(system_prompt.as_deref(), Some(expected_system));
    assert_eq!(initial_user_message, expected_user);
    assert_eq!(tools, &expected_tools);
    for (actual, expected) in tools.iter().zip(&expected_tools) {
        assert_eq!(actual.parameters, expected.parameters);
    }

    // The exact same startup values reached the provider request context.
    let request: serde_json::Value =
        serde_json::from_slice(&fake.requests()[0].body).expect("provider request JSON");
    assert_eq!(request["messages"][0]["content"], expected_system);
    assert_eq!(request["messages"][1]["content"], expected_user);
    let provider_tools = request["tools"].as_array().expect("provider tools");
    assert_eq!(provider_tools.len(), expected_tools.len());
    for (actual, expected) in provider_tools.iter().zip(&expected_tools) {
        assert_eq!(actual["function"]["name"], expected.name);
        assert_eq!(actual["function"]["description"], expected.description);
        assert_eq!(actual["function"]["parameters"], expected.parameters);
    }

    // Lifecycle events present.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnStart { .. })),
        "expected a TurnStart event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolStart { .. })),
        "expected a ToolStart event (the write tool)"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
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
fn panicking_event_sink_does_not_change_the_run_result() {
    use temper_agent_core::{EventSink, run_sub_agent_with_events};

    struct PanickingSink;
    impl EventSink for PanickingSink {
        fn emit(&self, _event: temper_agent_core::AgentEvent) {
            panic!("capture sink failed");
        }
    }

    let fake = FakeLlm::start(Script::Fixed(Reply {
        turns: vec![Turn::Text("done".to_string())],
        usage: Default::default(),
        stop: StopReason::Stop,
    }))
    .expect("start fake LLM");
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-panicking-event-sink",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
    .build_provider()
    .expect("build provider");

    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_sub_agent_with_events(
            handle,
            SubAgent {
                system_prompt: Some("System prompt".to_string()),
                user_message: "Finish normally".to_string(),
                tools: ToolRegistry::new(),
                max_iterations: 2,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
                provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
            Arc::new(PanickingSink),
        )
        .await
    })
    .expect("observability panic must not fail the run");

    assert_eq!(outcome.stop, AgentStop::Completed);
    assert!(outcome.final_message.content.iter().any(|block| {
        matches!(block, tongs::model::ContentBlock::Text(text) if text.text == "done")
    }));
}

#[test]
fn sub_agent_can_be_aborted_mid_run() {
    use temper_agent_core::{NullEventSink, run_sub_agent_controllable};

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

    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        // A high iteration budget so the run would otherwise spin for a long
        // time; abort is what ends it.
        let (control, run) = run_sub_agent_controllable(
            handle.clone(),
            SubAgent {
                system_prompt: None,
                user_message: "loop".to_string(),
                tools,
                max_iterations: 100,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
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
        handle.spawn_with_cx(move |cx| async move {
            // Let a couple of turns happen first (virtual time).
            skein::time::sleep(
                temper_agent_io::timer_now(&cx),
                std::time::Duration::from_millis(50),
            )
            .await;
            control.abort();
        });

        run.await
    })
    .expect("run resolves");

    assert_eq!(
        outcome.stop,
        AgentStop::Aborted,
        "abort should stop the run"
    );
    // It did not run anywhere near the 100-iteration budget.
    assert!(
        fake.requests().len() < 100,
        "abort should stop the loop well before the budget"
    );
}

#[test]
fn sub_agent_steering_reaches_the_model() {
    use std::sync::Mutex;
    use temper_agent_core::{NullEventSink, run_sub_agent_controllable};

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

    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        let (control, run) = run_sub_agent_controllable(
            handle,
            SubAgent {
                system_prompt: None,
                user_message: "do a thing".to_string(),
                tools,
                max_iterations: 10,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
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
        let path = std::env::temp_dir().join(format!("anvil-{name}-{}", std::process::id()));
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
