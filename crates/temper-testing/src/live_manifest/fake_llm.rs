use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use jig_core::{Reply, RequestView, Script, ScriptFile};
use jig_server::FakeLlm;
use serde_json::Value;

const ARCHITECT_ROLE_PROMPT: &str = "ROLE: architect";
const ENGINEER_ROLE_PROMPT: &str = "ROLE: engineer";

#[derive(Clone, PartialEq)]
struct FixtureExpectations {
    architect_body: String,
    engineer_summary: String,
}

static EXPECTATIONS: OnceLock<FixtureExpectations> = OnceLock::new();

pub(super) fn architect_body() -> &'static str {
    EXPECTATIONS
        .get()
        .expect("scenario-owned Jig script is loaded before convergence")
        .architect_body
        .as_str()
}

pub(super) fn engineer_summary() -> &'static str {
    EXPECTATIONS
        .get()
        .expect("scenario-owned Jig script is loaded before convergence")
        .engineer_summary
        .as_str()
}

/// Jig-compatible fake LLM backed by the script declared in the resolved bundle.
pub(super) struct SinglePullRequestFake {
    fake: FakeLlm,
    architect_requests: Arc<AtomicUsize>,
    engineer_requests: Arc<AtomicUsize>,
}

impl SinglePullRequestFake {
    pub(super) fn start(path: &Path) -> Result<Self, String> {
        let expectations = expectations_from_path(path)?;
        if let Some(existing) = EXPECTATIONS.get() {
            if existing != &expectations {
                return Err(
                    "basic-delivery convergence cannot mix different Jig expectation scripts in one process"
                        .to_string(),
                );
            }
        } else {
            EXPECTATIONS
                .set(expectations)
                .map_err(|_| "failed to initialize Jig expectations".to_string())?;
        }
        let script = ScriptFile::load(path)
            .map_err(|error| format!("load scenario Jig script {}: {error}", path.display()))?
            .into_script();

        let architect_requests = Arc::new(AtomicUsize::new(0));
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let architect_seen = Arc::clone(&architect_requests);
        let engineer_seen = Arc::clone(&engineer_requests);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if messages_contain(view, ARCHITECT_ROLE_PROMPT) {
                architect_seen.fetch_add(1, Ordering::SeqCst);
            } else if messages_contain(view, ENGINEER_ROLE_PROMPT) {
                engineer_seen.fetch_add(1, Ordering::SeqCst);
            } else {
                return Reply::text("unexpected role for scenario-owned Jig script");
            }
            script.next_reply(view)
        }))
        .map_err(|error| format!("start scenario Jig fake LLM: {error}"))?;
        Ok(Self {
            fake,
            architect_requests,
            engineer_requests,
        })
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

fn expectations_from_path(path: &Path) -> Result<FixtureExpectations, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("read scenario Jig script {}: {error}", path.display()))?;
    let document: Value = serde_json::from_str(&source)
        .map_err(|error| format!("parse scenario Jig script {}: {error}", path.display()))?;
    let phases = document
        .get("phases")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("scenario Jig script {} must declare phases", path.display()))?;
    let architect = workspace_result_for_phase(phases, "architect-triage")?;
    let engineer = workspace_result_for_phase(phases, "engineer-implementation")?;
    Ok(FixtureExpectations {
        architect_body: required_string(&architect, "body", "architect-triage")?,
        engineer_summary: required_string(&engineer, "summary", "engineer-implementation")?,
    })
}

fn workspace_result_for_phase(phases: &[Value], name: &str) -> Result<Value, String> {
    let phase = phases
        .iter()
        .find(|phase| phase.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("scenario Jig script is missing `{name}` phase"))?;
    let replies = phase
        .get("sequence")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("scenario Jig phase `{name}` requires a sequence"))?;
    replies
        .iter()
        .rev()
        .find_map(|reply| reply.get("text").and_then(Value::as_str))
        .ok_or_else(|| format!("scenario Jig phase `{name}` requires a final text reply"))
        .and_then(|text| {
            serde_json::from_str(text)
                .map_err(|error| format!("scenario Jig phase `{name}` result is not JSON: {error}"))
        })
}

fn required_string(value: &Value, field: &str, phase: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("scenario Jig phase `{phase}` result is missing `{field}`"))
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn role_hint(view: &RequestView) -> &'static str {
    if messages_contain(view, ARCHITECT_ROLE_PROMPT) {
        "architect"
    } else if messages_contain(view, ENGINEER_ROLE_PROMPT) {
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
