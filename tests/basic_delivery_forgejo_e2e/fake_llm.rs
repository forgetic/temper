use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use jig_core::{
    PhaseSpec, Reply, ReplySpec, RequestView, Script, ScriptFile, StopSpec, TurnSpec, fixtures_root,
};
use jig_server::FakeLlm;
use serde_json::Value;

const BASIC_DELIVERY_FIXTURE: &str = "basic-delivery.json";
const ARCHITECT_PHASE: &str = "architect-triage";
const ENGINEER_PHASE: &str = "engineer-implementation";
const ARCHITECT_ROLE_PROMPT: &str = "ROLE: architect";
const ENGINEER_ROLE_PROMPT: &str = "ROLE: engineer";

struct FixtureExpectations {
    architect_body: String,
    engineer_summary: String,
}

pub(super) fn architect_body() -> &'static str {
    fixture_expectations().architect_body.as_str()
}

pub(super) fn engineer_summary() -> &'static str {
    fixture_expectations().engineer_summary.as_str()
}

/// Jig-compatible fake LLM backed by jig's canonical `basic-delivery.json`.
pub(super) struct BasicDeliveryFake {
    fake: FakeLlm,
    architect_requests: Arc<AtomicUsize>,
    engineer_requests: Arc<AtomicUsize>,
}

impl BasicDeliveryFake {
    pub(super) fn start() -> Self {
        let path = basic_delivery_fixture_path();
        let script_file = load_script_file(&path);
        let _expectations = expect_fixture_expectations(&script_file, &path);

        let architect_requests = Arc::new(AtomicUsize::new(0));
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let architect_seen = Arc::clone(&architect_requests);
        let engineer_seen = Arc::clone(&engineer_requests);
        let script = script_file.into_script();
        let fake = FakeLlm::start(Script::rule(move |view| {
            let is_architect = messages_contain(view, ARCHITECT_ROLE_PROMPT);
            let is_engineer = messages_contain(view, ENGINEER_ROLE_PROMPT);
            if is_architect {
                architect_seen.fetch_add(1, Ordering::SeqCst);
            }
            if is_engineer {
                engineer_seen.fetch_add(1, Ordering::SeqCst);
            }
            if is_architect || is_engineer {
                script.next_reply(view)
            } else {
                Reply::text(format!(
                    "unexpected basic-delivery fake-LLM request; canonical fixture is {BASIC_DELIVERY_FIXTURE}"
                ))
            }
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

fn fixture_expectations() -> &'static FixtureExpectations {
    static EXPECTATIONS: OnceLock<FixtureExpectations> = OnceLock::new();
    EXPECTATIONS.get_or_init(|| {
        let path = basic_delivery_fixture_path();
        let script_file = load_script_file(&path);
        expect_fixture_expectations(&script_file, &path)
    })
}

fn basic_delivery_fixture_path() -> PathBuf {
    fixtures_root().join(BASIC_DELIVERY_FIXTURE)
}

fn load_script_file(path: &Path) -> ScriptFile {
    ScriptFile::load(path).unwrap_or_else(|error| {
        panic!(
            "load canonical jig basic-delivery fixture {}: {error}",
            path.display()
        )
    })
}

fn expect_fixture_expectations(script_file: &ScriptFile, path: &Path) -> FixtureExpectations {
    expectations_from_script_file(script_file).unwrap_or_else(|error| {
        panic!(
            "canonical jig fixture {} no longer matches the basic_delivery_forgejo_e2e contract: {error}",
            path.display()
        )
    })
}

fn expectations_from_script_file(script_file: &ScriptFile) -> Result<FixtureExpectations, String> {
    let phases = match script_file {
        ScriptFile::Phases(phases) => phases.as_slice(),
        other => {
            return Err(format!(
                "expected a phase script with `{ARCHITECT_PHASE}` and `{ENGINEER_PHASE}` phases, got {other:?}"
            ));
        }
    };

    let architect_phase = phase_by_name(phases, ARCHITECT_PHASE)?;
    require_role_matcher(architect_phase, ARCHITECT_ROLE_PROMPT)?;
    require_two_step_phase(architect_phase)?;
    require_tool_call_reply(reply_at(architect_phase, 0)?, ARCHITECT_PHASE, "bash")?;
    let architect_result =
        workspace_result(reply_text(reply_at(architect_phase, 1)?)?, ARCHITECT_PHASE)?;
    require_string_field(&architect_result, "verdict", ARCHITECT_PHASE).and_then(|verdict| {
        if verdict == "ready_code" {
            Ok(())
        } else {
            Err(format!(
                "`{ARCHITECT_PHASE}` result verdict {verdict:?} is not `ready_code`"
            ))
        }
    })?;
    let architect_body = require_string_field(&architect_result, "body", ARCHITECT_PHASE)?;

    let engineer_phase = phase_by_name(phases, ENGINEER_PHASE)?;
    require_role_matcher(engineer_phase, ENGINEER_ROLE_PROMPT)?;
    require_two_step_phase(engineer_phase)?;
    require_tool_call_reply(reply_at(engineer_phase, 0)?, ENGINEER_PHASE, "write")?;
    let engineer_result =
        workspace_result(reply_text(reply_at(engineer_phase, 1)?)?, ENGINEER_PHASE)?;
    if engineer_result.get("verdict").is_some() {
        return Err(format!(
            "`{ENGINEER_PHASE}` result must be a success-path WorkspaceResult without a verdict"
        ));
    }
    let engineer_summary = require_string_field(&engineer_result, "summary", ENGINEER_PHASE)?;

    Ok(FixtureExpectations {
        architect_body,
        engineer_summary,
    })
}

fn phase_by_name<'a>(phases: &'a [PhaseSpec], name: &str) -> Result<&'a PhaseSpec, String> {
    phases
        .iter()
        .find(|phase| phase.name == name)
        .ok_or_else(|| format!("missing `{name}` phase in canonical fixture"))
}

fn require_role_matcher(phase: &PhaseSpec, role_prompt: &str) -> Result<(), String> {
    let expected = vec![role_prompt.to_string()];
    if phase.when.messages_contain == expected
        && phase.when.any_message_contains.is_empty()
        && phase.when.last_message_contains.is_empty()
        && phase.when.prior_tool_results.is_none()
        && phase.when.model.is_none()
        && phase.when.dialect.is_none()
        && !phase.when.ignore_case
    {
        Ok(())
    } else {
        Err(format!(
            "`{}` phase must be selected only by messages_contain={expected:?}, got {:?}",
            phase.name, phase.when
        ))
    }
}

fn require_two_step_phase(phase: &PhaseSpec) -> Result<(), String> {
    if phase.sequence.len() == 2 {
        Ok(())
    } else {
        Err(format!(
            "`{}` phase must serve exactly two replies (tool call, then WorkspaceResult), got {}",
            phase.name,
            phase.sequence.len()
        ))
    }
}

fn reply_at(phase: &PhaseSpec, index: usize) -> Result<&ReplySpec, String> {
    phase.sequence.get(index).ok_or_else(|| {
        format!(
            "`{}` phase is missing reply at sequence index {index}",
            phase.name
        )
    })
}

fn require_tool_call_reply(
    reply: &ReplySpec,
    phase: &str,
    expected_tool: &str,
) -> Result<(), String> {
    let ReplySpec::Full { turns, stop, .. } = reply else {
        return Err(format!(
            "`{phase}` first reply must use the full form for a tool-call stop, got {reply:?}"
        ));
    };
    if *stop != StopSpec::ToolCalls {
        return Err(format!(
            "`{phase}` first reply must stop with tool_calls, got {stop:?}"
        ));
    }
    let [TurnSpec::ToolCall(tool)] = turns.as_slice() else {
        return Err(format!(
            "`{phase}` first reply must contain exactly one tool call, got {turns:?}"
        ));
    };
    if tool.name == expected_tool {
        Ok(())
    } else {
        Err(format!(
            "`{phase}` first reply must call tool {expected_tool:?}, got {:?}",
            tool.name
        ))
    }
}

fn reply_text(reply: &ReplySpec) -> Result<&str, String> {
    match reply {
        ReplySpec::Text { text } => Ok(text.as_str()),
        ReplySpec::Full { turns, stop, .. } => {
            if *stop != StopSpec::Stop {
                return Err(format!(
                    "WorkspaceResult reply must stop normally, got {stop:?}"
                ));
            }
            match turns.as_slice() {
                [TurnSpec::Text(text)] => Ok(text.as_str()),
                _ => Err(format!(
                    "WorkspaceResult reply must contain exactly one text turn, got {turns:?}"
                )),
            }
        }
    }
}

fn workspace_result(text: &str, phase: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if trimmed != text {
        return Err(format!(
            "`{phase}` WorkspaceResult must be a single JSON object without surrounding prose or whitespace"
        ));
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|error| format!("`{phase}` WorkspaceResult does not parse as JSON: {error}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(format!(
            "`{phase}` WorkspaceResult must be a JSON object, got {value}"
        ))
    }
}

fn require_string_field(value: &Value, field: &str, phase: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("`{phase}` WorkspaceResult is missing string field `{field}`"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_fixture_matches_basic_delivery_e2e_contract() {
        let path = basic_delivery_fixture_path();
        let script_file = load_script_file(&path);
        let expectations = expect_fixture_expectations(&script_file, &path);
        assert!(
            !expectations.architect_body.trim().is_empty(),
            "architect body must be derived from the canonical jig fixture"
        );
        assert!(
            !expectations.engineer_summary.trim().is_empty(),
            "engineer summary must be derived from the canonical jig fixture"
        );
    }
}
