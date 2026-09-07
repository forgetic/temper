//! Two issue-specific model gates; no cancellation or Forge authority lives here.
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use jig_core::{Reply, RequestView, Script};
use jig_server::FakeLlm;
use serde_json::{Value, json};

use super::super::mapped_focused_test_relevance_fake::{provider_results, tool_reply};
use super::super::messages_contain;

#[derive(Default)]
pub(in crate::live_manifest) struct Control {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    a_arrived: bool,
    b_arrived: bool,
    release_a: bool,
    release_b: bool,
    sentinel_consumed: bool,
    late_a_result: bool,
    submitted: bool,
    failure: Option<String>,
}

impl Control {
    pub(super) fn gate(&self, a: bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut state = self.state.lock().expect("lifecycle gate");
        if a {
            state.a_arrived = true;
        } else {
            state.b_arrived = true;
        }
        self.wake.notify_all();
        while !(if a { state.release_a } else { state.release_b }) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.failure = Some("model gate deadline expired".into());
                return false;
            }
            state = self
                .wake
                .wait_timeout(state, remaining)
                .expect("lifecycle gate")
                .0;
        }
        true
    }

    pub(super) fn arrived(&self, a: bool) -> bool {
        let state = self.state.lock().expect("lifecycle gate");
        if a { state.a_arrived } else { state.b_arrived }
    }

    pub(super) fn release(&self, a: bool) {
        let mut state = self.state.lock().expect("lifecycle gate");
        if a {
            state.release_a = true;
        } else {
            state.release_b = true;
        }
        self.wake.notify_all();
    }

    pub(super) fn healthy(&self) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "lifecycle gate poisoned")?
            .failure
            .clone()
            .map_or(Ok(()), Err)
    }

    pub(super) fn fail(&self, message: String) {
        self.state.lock().expect("lifecycle gate").failure = Some(message);
    }

    pub(super) fn completed(&self) -> Result<(), String> {
        self.healthy()?;
        let state = self.state.lock().map_err(|_| "lifecycle gate poisoned")?;
        if state.sentinel_consumed && state.submitted && state.late_a_result {
            Ok(())
        } else {
            Err("B did not consume the exact post-cancellation source and submit".into())
        }
    }
}

pub(in crate::live_manifest::codebase_memory) fn start(
    count: Arc<AtomicUsize>,
    control: Arc<Control>,
) -> Result<(FakeLlm, super::JigRouter), String> {
    let model_control = Arc::clone(&control);
    let fake = FakeLlm::start(Script::rule(move |view| {
        count.fetch_add(1, Ordering::SeqCst);
        match reply(view, &model_control) {
            Ok(reply) => reply,
            Err(error) => {
                model_control.fail(error);
                Reply::text("Lifecycle fixture rejected an unexpected model/tool transition.")
            }
        }
    }))
    .map_err(|error| format!("start shared lifecycle Jig: {error}"))?;
    let router = super::JigRouter::start(&fake.base_url(), control)?;
    Ok((fake, router))
}

fn reply(view: &RequestView, control: &Control) -> Result<Reply, String> {
    if !messages_contain(view, "ROLE: engineer") {
        return Err("unexpected lifecycle role".into());
    }
    if messages_contain(view, "LIFECYCLE_JOB_A") {
        if view.prior_tool_results != 0 || !control.arrived(true) {
            return Err("unexpected A model transition".into());
        }
        control.state.lock().expect("lifecycle gate").late_a_result = true;
        return Ok(Reply::text(
            r##"{"title":"Late withdrawn A result","body":"# Implementation report\nWithdrawn A must never publish this result.","summary":"LIFECYCLE_LATE_A_RESULT"}"##,
        ));
    }
    if !messages_contain(view, "LIFECYCLE_JOB_B") {
        return Err("missing lifecycle issue identity".into());
    }
    let turn = view.prior_tool_results;
    if turn == 0 && !control.arrived(false) {
        return Err("B bypassed its HTTP gate".into());
    }
    if turn > 0
        && view
            .messages
            .iter()
            .filter(|message| message.role == "tool")
            .any(|message| message.content.len() > 20 * 1024)
    {
        return Err("oversized lifecycle tool result".into());
    }
    Ok(match turn {
        0 => tool_reply(
            "lifecycle-b-post-cancel-search",
            "codebase_memory_search_graph",
            json!({"query":"retry worker affinity","limit":4}),
        ),
        1 => source(view, "implementation", "/results/0/qualified_name")?,
        2 => {
            exact_source(
                view,
                "src/lib.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scenarios/shared-codebase-memory-lifecycle/repo/src/lib.rs"
                )),
            )?;
            control
                .state
                .lock()
                .expect("lifecycle gate")
                .sentinel_consumed = true;
            tool_reply(
                "lifecycle-b-caller-trace",
                "codebase_memory_trace_path",
                json!({"function_name":selected(view,"/results/0/qualified_name")?,"direction":"inbound"}),
            )
        }
        3 => source(view, "caller", "/callers/0/qualified_name")?,
        4 => source(view, "focused_test", "/results/1/qualified_name")?,
        5 => {
            exact_source(
                view,
                "src/caller.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scenarios/shared-codebase-memory-lifecycle/repo/src/caller.rs"
                )),
            )?;
            exact_source(
                view,
                "tests/retry_affinity.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scenarios/shared-codebase-memory-lifecycle/repo/tests/retry_affinity.rs"
                )),
            )?;
            tool_reply(
                "lifecycle-b-ordinary-read",
                "read",
                json!({"path":"demo/src/lib.rs"}),
            )
        }
        6 => tool_reply(
            "lifecycle-b-repair",
            "write",
            json!({"path":"demo/src/lib.rs","content":"pub mod caller;\n\npub fn retry_worker_topic<'a>(\n    topic: &'a str,\n    canonical_topic: Option<&'a str>,\n    _attempt: u32,\n) -> &'a str {\n    canonical_topic.unwrap_or(topic)\n}\n"}),
        ),
        7 => tool_reply(
            "lifecycle-b-validate",
            "bash",
            json!({"command":"cd demo && cargo fmt --check && cargo test --quiet && git diff --check","timeout":60}),
        ),
        8 => tool_reply(
            "lifecycle-b-submit",
            "submit_for_pr",
            json!({"summary":"Used codebase-memory graph evidence, then validated the retry-worker repair."}),
        ),
        9 => {
            if !view
                .messages
                .iter()
                .rev()
                .find(|message| message.role == "tool")
                .is_some_and(|message| {
                    message
                        .content
                        .starts_with("submit_for_pr accepted by host: ")
                })
            {
                return Err("B host submission was not accepted".into());
            }
            control.state.lock().expect("lifecycle gate").submitted = true;
            Reply::text(
                r##"{"title":"Preserve retry affinity after shared provider cancellation","body":"# Implementation report\nUsed codebase-memory graph evidence, then validated the retry-worker repair.","summary":"B consumed the exact source after A cleanup and validated the repair."}"##,
            )
        }
        _ => return Err("unexpected B model turn".into()),
    })
}

fn selected(view: &RequestView, pointer: &str) -> Result<String, String> {
    provider_results(view)
        .iter()
        .find_map(|result| result.pointer(pointer).and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| "missing returned lifecycle selector".into())
}

fn source(view: &RequestView, kind: &str, pointer: &str) -> Result<Reply, String> {
    Ok(tool_reply(
        &format!("lifecycle-b-source-{kind}"),
        "codebase_memory_get_code_snippet",
        json!({"qualified_name":selected(view,pointer)?,"decision_evidence_kind":kind}),
    ))
}

fn exact_source(view: &RequestView, path: &str, expected: &str) -> Result<(), String> {
    if provider_results(view).iter().any(|result| {
        result["file_path"] == path
            && result["source"] == expected
            && result["qualified_name"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }) {
        Ok(())
    } else {
        Err(format!("B omitted successful exact source for {path}"))
    }
}
