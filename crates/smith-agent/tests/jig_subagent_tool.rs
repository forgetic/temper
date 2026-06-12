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
use std::sync::Arc;

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use pi::provider::StreamOptions;
use pi::sdk::create_read_tool;
use pi::tools::{ToolEffects, ToolRegistry};
use smith_agent::{AgentStop, SubAgent, SubAgentTool, run_sub_agent};
use smith_temper_agent::ProviderConfig;

#[test]
fn parent_agent_delegates_to_a_sub_agent() {
    let checkout = TempCheckout::new("subagent-tool");
    fs::write(
        checkout.path().join("FACTS.md"),
        "the answer is 42\n",
    )
    .expect("seed FACTS.md");

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
    let factory: smith_agent::SubAgentFactory = Arc::new(move |task: String| {
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
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        }
    });

    // The investigate tool is read-only ⇒ parallel-safe (a parent could fan out
    // several at once).
    let investigate = SubAgentTool::new(
        "investigate",
        "Delegate a read-only investigation to a sub-agent. Input: { task }.",
        ToolEffects::read(),
        factory,
    );

    let parent_provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-parent",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(parent_fake.base_url())
    .build_provider()
    .expect("build parent provider");

    let outcome = smith_io_engine::block_on(async move {
        run_sub_agent(SubAgent {
            system_prompt: Some("You are an orchestrator. Use the investigate tool.".into()),
            user_message: "What is the answer? Use the investigate sub-agent.".into(),
            tools: ToolRegistry::from_tools(vec![Box::new(investigate)]),
            max_iterations: 4,
            provider: parent_provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        })
        .await
    })
    .expect("parent agent runs");

    assert_eq!(outcome.stop, AgentStop::Completed);

    // The parent's conversation contains the sub-agent's finding as a tool
    // result.
    let tool_results: Vec<String> = outcome
        .messages
        .iter()
        .filter_map(|m| match m {
            pi::model::Message::ToolResult(r) => Some(
                r.content
                    .iter()
                    .filter_map(|b| match b {
                        pi::model::ContentBlock::Text(t) => Some(t.text.clone()),
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
    assert!(parent_fake.requests().len() >= 2, "parent should loop after the tool");
    assert!(sub_fake.requests().len() >= 2, "sub-agent should run its own tool loop");
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
    let factory: smith_agent::SubAgentFactory = Arc::new(move |task: String| {
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
            provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        }
    });

    let investigate = SubAgentTool::new(
        "investigate",
        "Read-only investigation. Input: { task }.",
        ToolEffects::read(),
        factory,
    );

    let parent_provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-parent",
        "https://example.invalid/unused",
        "sk-jig-test",
    )
    .with_base_url_override(parent_fake.base_url())
    .build_provider()
    .expect("build parent provider");

    let outcome = smith_io_engine::block_on(async move {
        run_sub_agent(SubAgent {
            system_prompt: Some("Orchestrator.".into()),
            user_message: "Investigate A and B.".into(),
            tools: ToolRegistry::from_tools(vec![Box::new(investigate)]),
            max_iterations: 4,
            provider: parent_provider,
            stream_options: StreamOptions {
                api_key: Some("sk-jig-test".to_string()),
                ..StreamOptions::default()
            },
        })
        .await
    })
    .expect("parent runs");

    assert_eq!(outcome.stop, AgentStop::Completed);
    // Both sub-agent tool calls produced results in the parent conversation.
    let tool_result_count = outcome
        .messages
        .iter()
        .filter(|m| matches!(m, pi::model::Message::ToolResult(_)))
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
