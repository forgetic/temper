use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;

pub(super) const ARCHITECT_BODY: &str = "## Code spec\n\nImplement: Service banner should identify the environment\n\n### Requirements\n\n- Add or update `service/src/banner.py` with a `service_banner(environment=None, greeting=None)` helper.\n- Include the active environment in the returned banner.\n- Default `environment` from `SERVICE_ENVIRONMENT`, falling back to `development`.\n- Default `greeting` from `SERVICE_BANNER_GREETING`, falling back to `Hello`.\n";

pub(super) const ENGINEER_SUMMARY: &str =
    "Updated service/src/banner.py with an environment-aware service_banner helper.";

const ENGINEER_FILE: &str = "\"\"\"Service banner helpers.\"\"\"\n\nfrom __future__ import annotations\n\nimport os\n\n\ndef service_banner(environment=None, greeting=None):\n    \"\"\"Return a greeting that identifies the active service environment.\"\"\"\n    active_environment = (\n        environment\n        if environment is not None\n        else os.getenv(\"SERVICE_ENVIRONMENT\", \"development\")\n    )\n    active_greeting = (\n        greeting\n        if greeting is not None\n        else os.getenv(\"SERVICE_BANNER_GREETING\", \"Hello\")\n    )\n    return f\"{active_greeting} from the {active_environment} environment\"\n\n\n__all__ = [\"service_banner\"]\n";

/// Jig-compatible fake LLM mirroring `jig/fixtures/basic-delivery.json`.
pub(super) struct BasicDeliveryFake {
    fake: FakeLlm,
    architect_requests: Arc<AtomicUsize>,
    engineer_requests: Arc<AtomicUsize>,
}

impl BasicDeliveryFake {
    pub(super) fn start() -> Self {
        let architect_requests = Arc::new(AtomicUsize::new(0));
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let architect_seen = Arc::clone(&architect_requests);
        let engineer_seen = Arc::clone(&engineer_requests);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if messages_contain(view, "ROLE: architect") {
                architect_seen.fetch_add(1, Ordering::SeqCst);
                return architect_reply(view);
            }
            if messages_contain(view, "ROLE: engineer") {
                engineer_seen.fetch_add(1, Ordering::SeqCst);
                return engineer_reply(view);
            }
            Reply::text("unexpected basic-delivery fake-LLM request")
        }))
        .expect("start basic-delivery fake LLM");
        Self {
            fake,
            architect_requests,
            engineer_requests,
        }
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn architect_requests(&self) -> usize {
        self.architect_requests.load(Ordering::SeqCst)
    }

    pub(super) fn engineer_requests(&self) -> usize {
        self.engineer_requests.load(Ordering::SeqCst)
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        if requests.is_empty() {
            return "<fake LLM received no requests>".to_string();
        }
        let start = requests.len().saturating_sub(16);
        requests[start..]
            .iter()
            .enumerate()
            .map(|(offset, request)| {
                let index = start + offset + 1;
                let view = request.view.as_ref();
                let role = view.map(role_hint).unwrap_or("unknown");
                let prior = view.map(|v| v.prior_tool_results).unwrap_or_default();
                let last = view
                    .and_then(RequestView::last_message)
                    .map(|m| format!("{}: {}", m.role, snippet(&m.content, 160)))
                    .unwrap_or_else(|| "<no projected message>".to_string());
                format!(
                    "#{index} {} {} role={role} prior_tool_results={prior} last={last}",
                    request.method, request.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn architect_reply(view: &RequestView) -> Reply {
    if view.prior_tool_results == 0 {
        std::thread::sleep(Duration::from_millis(1_000));
        Reply {
            turns: vec![Turn::ToolCall {
                id: "call_architect_inspect".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({
                    "command": "printf 'jig-basic-delivery-inspection\\n'; if [ -d service ]; then find service -maxdepth 2 -type f -print | sort; else printf 'service/ not present\\n'; fi"
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        }
    } else {
        Reply::text(
            serde_json::json!({
                "verdict": "ready_code",
                "body": ARCHITECT_BODY,
            })
            .to_string(),
        )
    }
}

fn engineer_reply(view: &RequestView) -> Reply {
    if view.prior_tool_results == 0 {
        std::thread::sleep(Duration::from_millis(2_000));
        Reply {
            turns: vec![Turn::ToolCall {
                id: "call_engineer_write_banner".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({
                    "path": "service/src/banner.py",
                    "content": ENGINEER_FILE,
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        }
    } else {
        Reply::text(
            serde_json::json!({
                "summary": ENGINEER_SUMMARY,
            })
            .to_string(),
        )
    }
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn role_hint(view: &RequestView) -> &'static str {
    if messages_contain(view, "ROLE: architect") {
        "architect"
    } else if messages_contain(view, "ROLE: engineer") {
        "engineer"
    } else {
        "unknown"
    }
}

fn snippet(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            out.push('…');
            break;
        }
        out.push(if ch == '\n' { ' ' } else { ch });
    }
    out
}
