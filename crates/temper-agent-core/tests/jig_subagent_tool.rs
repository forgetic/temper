//! Jig-backed e2e: a parent agent delegates to a sub-agent via a [`SubAgentTool`].
//!
//! Two fake LLMs: the parent's provider talks to `parent_fake`, the sub-agent's
//! to `sub_fake`. The parent model calls the `investigate` sub-agent tool; that
//! runs a nested sub-agent (its own AgentMachine + shell) which reads a file and
//! reports a finding; the finding flows back into the parent's conversation as a
//! tool result, and the parent then completes. Exercises the full nesting:
//! parent loop → sub-agent tool → nested loop → result back up.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::ProviderConfig;
use temper_agent_core::{AgentEvent, AgentStop, EventSink, SubAgent, SubAgentTool, run_sub_agent};
use tongs::provider::StreamOptions;
use tongs::tools::create_read_tool;
use tongs::tools::{Tool, ToolEffects, ToolRegistry};

#[derive(Default)]
struct EventRecorder(Mutex<Vec<AgentEvent>>);

impl EventSink for EventRecorder {
    fn emit(&self, event: AgentEvent) {
        self.0.lock().expect("event recorder").push(event);
    }
}

#[test]
fn parent_agent_delegates_to_a_sub_agent() {
    let checkout = TempCheckout::new("subagent-tool");
    fs::write(checkout.path().join("FACTS.md"), "the answer is 42\n").expect("seed FACTS.md");

    // Sub-agent fake: reads FACTS.md, then reports the finding.
    let sub_fake = FakeLlm::start(Script::rule(|view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_read".to_string(),
                    name: "read".to_string(),
                    args: serde_json::json!({ "path": "FACTS.md" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            Reply::text("The file says the answer is 42.")
        }
    }))
    .expect("start sub fake");

    // Parent fake: calls the investigate sub-agent, then completes using its
    // result.
    let parent_fake = FakeLlm::start(Script::rule(|view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_investigate".to_string(),
                    name: "investigate".to_string(),
                    args: serde_json::json!({ "task": "find the answer in FACTS.md" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            Reply::text("Done: the sub-agent found the answer.")
        }
    }))
    .expect("start parent fake");

    // The sub-agent factory: each invocation builds a fresh read-only sub-agent
    // scoped to the checkout, talking to sub_fake.
    let sub_base_url = sub_fake.base_url();
    let checkout_path = checkout.path().to_path_buf();
    let factory: temper_agent_core::SubAgentFactory = Arc::new(move |task: String| {
        let provider = ProviderConfig::new(
            "jig-openai-compatible",
            "jig-sub",
            "https://example.invalid/unused",
            "sk-jig-test",
        )
        .with_base_url_override(sub_base_url.clone())
        .build_provider()
        .expect("build sub provider");
        SubAgent {
            system_prompt: Some("You are an investigator. Read files and report findings.".into()),
            user_message: task,
            tools: ToolRegistry::from_tools(vec![create_read_tool(&checkout_path)]),
            max_iterations: 4,
            operation_limits: temper_agent_core::AgentOperationLimits::default(),
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        }
    });

    let parent_provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-parent",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(parent_fake.base_url())
    .build_provider()
    .expect("build parent provider");

    let nested_events = Arc::new(EventRecorder::default());
    let nested_events_for_run = Arc::clone(&nested_events);
    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        // The investigate tool is read-only ⇒ parallel-safe (a parent could fan
        // out several at once). Built inside the engine task so it holds the
        // runtime handle explicitly for its nested runs.
        let investigate = SubAgentTool::new(
            handle.clone(),
            "investigate",
            "Delegate a read-only investigation to a sub-agent. Input: { task }.",
            ToolEffects::read(),
            factory,
        )
        .with_events(nested_events_for_run);
        run_sub_agent(
            handle,
            SubAgent {
                system_prompt: Some("You are an orchestrator. Use the investigate tool.".into()),
                user_message: "What is the answer? Use the investigate sub-agent.".into(),
                tools: ToolRegistry::from_tools(vec![Box::new(investigate)]),
                max_iterations: 4,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
                provider: parent_provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
        )
        .await
    })
    .expect("parent agent runs");

    assert_eq!(outcome.stop, AgentStop::Completed);

    let nested_events = nested_events.0.lock().expect("nested events");
    let prompt_count = nested_events
        .iter()
        .filter(|event| matches!(event, AgentEvent::PromptPrepared { .. }))
        .count();
    assert_eq!(
        prompt_count, 1,
        "one prompt event for the nested invocation"
    );
    let prompt_position = nested_events
        .iter()
        .position(|event| matches!(event, AgentEvent::PromptPrepared { .. }))
        .expect("nested prompt event");
    let turn_position = nested_events
        .iter()
        .position(|event| matches!(event, AgentEvent::TurnStart { .. }))
        .expect("nested turn event");
    assert!(prompt_position < turn_position);
    drop(nested_events);

    // The parent's conversation contains the sub-agent's finding as a tool
    // result.
    let tool_results: Vec<String> = outcome
        .messages
        .iter()
        .filter_map(|m| match m {
            tongs::model::Message::ToolResult(r) => Some(
                r.content
                    .iter()
                    .filter_map(|b| match b {
                        tongs::model::ContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        tool_results.iter().any(|r| r.contains("42")),
        "the sub-agent's finding (42) should appear as a parent tool result; got {tool_results:?}"
    );

    // Both fakes were exercised (parent did a tool round; sub did its own loop).
    assert!(
        parent_fake.requests().len() >= 2,
        "parent should loop after the tool"
    );
    assert!(
        sub_fake.requests().len() >= 2,
        "sub-agent should run its own tool loop"
    );
}

#[test]
fn parent_fans_out_two_sub_agents_in_one_batch() {
    let checkout = TempCheckout::new("subagent-fanout");
    fs::write(checkout.path().join("A.md"), "alpha\n").expect("seed A");
    fs::write(checkout.path().join("B.md"), "beta\n").expect("seed B");

    // Each sub-agent reads the file named in its task and reports it.
    let sub_fake = FakeLlm::start(Script::rule(|view| {
        if view.prior_tool_results == 0 {
            // The task text names the file; echo a fixed read of it. (The jig
            // view doesn't expose the user text here, so both sub-agents read
            // whichever file their task asked for via the tool args the model
            // emits — we just emit a read of the path embedded in the system.)
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "r".to_string(),
                    name: "read".to_string(),
                    args: serde_json::json!({ "path": "A.md" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            Reply::text("investigated")
        }
    }))
    .expect("start sub fake");

    // Parent emits TWO investigate calls in a single turn ⇒ one parallel batch
    // (both read-only). They must run concurrently and both complete before the
    // parent re-calls the model.
    let parent_fake = FakeLlm::start(Script::rule(|view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![
                    Turn::ToolCall {
                        id: "inv-a".to_string(),
                        name: "investigate".to_string(),
                        args: serde_json::json!({ "task": "read A.md" }),
                    },
                    Turn::ToolCall {
                        id: "inv-b".to_string(),
                        name: "investigate".to_string(),
                        args: serde_json::json!({ "task": "read B.md" }),
                    },
                ],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            Reply::text("both done")
        }
    }))
    .expect("start parent fake");

    let sub_base_url = sub_fake.base_url();
    let checkout_path = checkout.path().to_path_buf();
    let factory: temper_agent_core::SubAgentFactory = Arc::new(move |task: String| {
        let provider = ProviderConfig::new(
            "jig-openai-compatible",
            "jig-sub",
            "https://example.invalid/unused",
            "sk-jig-test",
        )
        .with_base_url_override(sub_base_url.clone())
        .build_provider()
        .expect("build sub provider");
        SubAgent {
            system_prompt: Some("Investigator.".into()),
            user_message: task,
            tools: ToolRegistry::from_tools(vec![create_read_tool(&checkout_path)]),
            max_iterations: 4,
            operation_limits: temper_agent_core::AgentOperationLimits::default(),
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        }
    });

    let parent_provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-parent",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(parent_fake.base_url())
    .build_provider()
    .expect("build parent provider");

    let outcome = temper_agent_io::block_on_with(move |_cx, handle| async move {
        let investigate = SubAgentTool::new(
            handle.clone(),
            "investigate",
            "Read-only investigation. Input: { task }.",
            ToolEffects::read(),
            factory,
        );
        run_sub_agent(
            handle,
            SubAgent {
                system_prompt: Some("Orchestrator.".into()),
                user_message: "Investigate A and B.".into(),
                tools: ToolRegistry::from_tools(vec![Box::new(investigate)]),
                max_iterations: 4,
                operation_limits: temper_agent_core::AgentOperationLimits::default(),
                provider: parent_provider,
                stream_options: StreamOptions {
                    api_key: Some("sk-jig-test".to_string()),
                    ..StreamOptions::default()
                },
            },
        )
        .await
    })
    .expect("parent runs");

    assert_eq!(outcome.stop, AgentStop::Completed);
    // Both sub-agent tool calls produced results in the parent conversation.
    let tool_result_count = outcome
        .messages
        .iter()
        .filter(|m| matches!(m, tongs::model::Message::ToolResult(_)))
        .count();
    assert_eq!(
        tool_result_count, 2,
        "both fanned-out sub-agents should produce a parent tool result"
    );
    // The sub-agent fake served two independent sub-agent runs (>= 2 turns each).
    assert!(
        sub_fake.requests().len() >= 4,
        "two sub-agents should each run their own loop"
    );
}

#[test]
fn nested_budget_exhaustion_is_a_failed_tool_result() {
    let sub_fake = FakeLlm::start(Script::Fixed(Reply {
        turns: vec![
            Turn::Text(r#"{"summary":"looks complete"}"#.to_string()),
            Turn::ToolCall {
                id: "undispatchable".to_string(),
                name: "read".to_string(),
                args: serde_json::json!({ "path": "NEVER_READ.md" }),
            },
        ],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }))
    .expect("start sub fake");
    let sub_base_url = sub_fake.base_url();
    let factory: temper_agent_core::SubAgentFactory = Arc::new(move |task: String| {
        let provider = ProviderConfig::new(
            "jig-openai-compatible",
            "jig-sub-budget",
            "https://example.invalid/unused",
            "sk-jig-test",
        )
        .with_base_url_override(sub_base_url.clone())
        .build_provider()
        .expect("build sub provider");
        SubAgent {
            system_prompt: Some("Return a nested result.".into()),
            user_message: task,
            tools: ToolRegistry::new(),
            max_iterations: 0,
            operation_limits: temper_agent_core::AgentOperationLimits::default(),
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        }
    });

    let output = temper_agent_io::block_on_with(move |_cx, handle| async move {
        let investigate = SubAgentTool::new(
            handle,
            "investigate",
            "Investigate a task.",
            ToolEffects::read(),
            factory,
        );
        investigate
            .execute(
                "call-investigate",
                serde_json::json!({ "task": "finish" }),
                None,
            )
            .await
    })
    .expect("nested tool executes");

    assert!(output.is_error, "budget exhaustion must be a tool error");
    assert_eq!(
        output.details.as_ref().and_then(|details| details
            .get("sub_agent_stop")
            .and_then(serde_json::Value::as_str)),
        Some("BudgetExhausted")
    );
    assert_eq!(sub_fake.requests().len(), 1);
}

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
