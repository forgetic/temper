use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::json;
use temper_workflow::{ArtifactKindId, WorkflowMetadata, render_metadata_block};

use super::{
    FEATURE_BRANCH, FEATURE_TITLE, FIRST_CODE_TITLE, LANDING_TITLE, PLAN_TITLE, SECOND_CODE_TITLE,
};

pub(super) struct PlanFeatureFake {
    fake: FakeLlm,
    architect_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    engineer_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tester_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl PlanFeatureFake {
    pub(super) fn start() -> Self {
        let architect_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let engineer_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tester_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let architect_seen = std::sync::Arc::clone(&architect_requests);
        let engineer_seen = std::sync::Arc::clone(&engineer_requests);
        let tester_seen = std::sync::Arc::clone(&tester_requests);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if request_role_is(view, "tester") {
                tester_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tester_reply(view)
            } else if request_role_is(view, "engineer") {
                engineer_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                engineer_reply(view)
            } else if request_role_is(view, "architect") {
                architect_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                architect_reply(view)
            } else {
                Reply::text("unexpected plan-centric fake-LLM request")
            }
        }))
        .expect("start plan-centric fake LLM");
        Self {
            fake,
            architect_requests,
            engineer_requests,
            tester_requests,
        }
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn architect_requests(&self) -> usize {
        self.architect_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn engineer_requests(&self) -> usize {
        self.engineer_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn tester_requests(&self) -> usize {
        self.tester_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        if requests.is_empty() {
            return "<fake LLM received no requests>".to_string();
        }
        let start = requests.len().saturating_sub(24);
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
        return bash_reply("printf plan-centric-architect\\n");
    }
    if messages_contain(view, PLAN_TITLE) || messages_contain(view, "decompose_plan") {
        Reply::text(
            json!({
                "verdict": "children_ready",
                "children": [
                    {
                        "slug": "foundation",
                        "kind": "code",
                        "title": FIRST_CODE_TITLE,
                        "body": "Implement the first feature-branch slice by adding the foundation fixture file. Keep the change small and self-contained."
                    },
                    {
                        "slug": "validation-landing",
                        "kind": "code",
                        "title": SECOND_CODE_TITLE,
                        "body": "Implement the validation and landing slice after the foundation slice has landed.",
                        "labels": ["blocked"],
                        "depends_on": ["foundation"]
                    }
                ]
            })
            .to_string(),
        )
    } else {
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("plan")),
            target_branch: Some(FEATURE_BRANCH.to_string()),
            ..WorkflowMetadata::default()
        };
        let body = format!(
            "Plan for {FEATURE_TITLE}.\n\nFeature branch: `{FEATURE_BRANCH}`.\n\nImplementation DAG:\n1. `{FIRST_CODE_TITLE}`.\n2. `{SECOND_CODE_TITLE}` after the first child lands.\n\nValidation: tester confirms both implementation PRs landed into the feature branch, then opens the aggregate landing PR.\n\n{}",
            render_metadata_block(&metadata)
        );
        Reply::text(
            json!({
                "verdict": "needs_plan",
                "children": [
                    {
                        "slug": "plan",
                        "kind": "plan",
                        "title": PLAN_TITLE,
                        "body": body
                    }
                ]
            })
            .to_string(),
        )
    }
}

fn engineer_reply(view: &RequestView) -> Reply {
    let second = messages_contain(view, SECOND_CODE_TITLE);
    match view.prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: if second { "call_write_second" } else { "call_write_first" }.to_string(),
                name: "write".to_string(),
                args: json!({
                    "path": if second { "service/VALIDATION_LANDING_SLICE.md" } else { "service/FOUNDATION_SLICE.md" },
                    "content": if second { "validation and landing slice\n" } else { "foundation slice\n" },
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: if second { "call_submit_second" } else { "call_submit_first" }.to_string(),
                name: "submit_for_pr".to_string(),
                args: json!({ "summary": if second { "Implemented validation and landing slice." } else { "Implemented foundation slice." } }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(
            json!({
                "title": if second { SECOND_CODE_TITLE } else { FIRST_CODE_TITLE },
                "body": if second { "Validation and landing slice implemented." } else { "Foundation slice implemented." },
                "summary": if second { "Implemented validation and landing slice." } else { "Implemented foundation slice." }
            })
            .to_string(),
        ),
    }
}

fn tester_reply(view: &RequestView) -> Reply {
    if view.prior_tool_results == 0 {
        return bash_reply("printf plan-centric-validation\\n");
    }
    Reply::text(
        json!({
            "verdict": "validated",
            "title": LANDING_TITLE,
            "body": "Validation passed for the current feature branch head. Both sequential implementation PRs landed into the feature branch, the downstream child unblocked after its prerequisite closed, and the aggregate branch is ready for main."
        })
        .to_string(),
    )
}

fn bash_reply(command: &str) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: "call_probe".to_string(),
            name: "bash".to_string(),
            args: json!({ "command": command }),
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn request_role_is(view: &RequestView, role: &str) -> bool {
    let title_case = format!("Role: {role}");
    let upper_case = format!("ROLE: {role}");
    view.messages.iter().any(|message| {
        message.content.contains(&title_case) || message.content.contains(&upper_case)
    })
}

fn role_hint(view: &RequestView) -> &'static str {
    if request_role_is(view, "tester") {
        "tester"
    } else if request_role_is(view, "engineer") {
        "engineer"
    } else if request_role_is(view, "architect") {
        "architect"
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
