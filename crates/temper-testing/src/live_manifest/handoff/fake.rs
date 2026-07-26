use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use jig_core::{Reply, RequestView, Script, ScriptFile};
use jig_server::FakeLlm;

use super::REFRESH_FAKE_TIMEOUT;

const REFRESH_TITLE_NEEDLE: &str = "refresh existing handoff";

pub(super) struct HandoffFake {
    fake: FakeLlm,
    state: Arc<(Mutex<FakeState>, Condvar)>,
    engineer_requests: Arc<AtomicUsize>,
}

#[derive(Default)]
struct FakeState {
    refresh_started: bool,
    refresh_pr_seeded: bool,
}

impl HandoffFake {
    pub(super) fn start(script_path: &Path) -> Result<Self, String> {
        let script = ScriptFile::load(script_path)
            .map_err(|error| {
                format!(
                    "load scenario Jig script {}: {error}",
                    script_path.display()
                )
            })?
            .into_script();
        let state = Arc::new((Mutex::new(FakeState::default()), Condvar::new()));
        let rule_state = Arc::clone(&state);
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&engineer_requests);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if !messages_contain(view, "ROLE: engineer") {
                return Reply::text("unexpected implementation-pr-handoff fake-LLM request");
            }
            request_count.fetch_add(1, Ordering::SeqCst);
            let refresh = messages_contain(view, REFRESH_TITLE_NEEDLE)
                || messages_contain(view, "Current implementation PR handoff");
            if refresh && view.prior_tool_results == 0 {
                let (lock, cvar) = &*rule_state;
                let mut state = lock.lock().expect("fake state lock");
                state.refresh_started = true;
                cvar.notify_all();
                let deadline = Instant::now() + REFRESH_FAKE_TIMEOUT;
                while !state.refresh_pr_seeded {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Reply::text(
                            "refresh fake timed out waiting for stale PR seed signal",
                        );
                    }
                    let (next, timeout) = cvar
                        .wait_timeout(state, remaining)
                        .expect("fake state condvar wait");
                    state = next;
                    if timeout.timed_out() && !state.refresh_pr_seeded {
                        return Reply::text(
                            "refresh fake timed out waiting for stale PR seed signal",
                        );
                    }
                }
            }
            script.next_reply(view)
        }))
        .map_err(|error| format!("start scenario Jig fake LLM: {error}"))?;
        Ok(Self {
            fake,
            state,
            engineer_requests,
        })
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn engineer_requests(&self) -> usize {
        self.engineer_requests.load(Ordering::SeqCst)
    }

    pub(super) fn wait_for_refresh_started(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().expect("fake state lock");
        while !state.refresh_started {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "fake LLM did not receive the refresh engineer request within {timeout:?}\n{}",
                    self.log_tail()
                ));
            }
            let (next, wait) = cvar
                .wait_timeout(state, remaining)
                .expect("fake state condvar wait");
            state = next;
            if wait.timed_out() && !state.refresh_started {
                return Err(format!(
                    "fake LLM did not receive the refresh engineer request within {timeout:?}\n{}",
                    self.log_tail()
                ));
            }
        }
        Ok(())
    }

    pub(super) fn allow_refresh_continue(&self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().expect("fake state lock");
        state.refresh_pr_seeded = true;
        cvar.notify_all();
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        if requests.is_empty() {
            return "<fake LLM received no requests>".to_string();
        }
        let start = requests.len().saturating_sub(20);
        requests[start..]
            .iter()
            .enumerate()
            .map(|(offset, request)| {
                let index = start + offset + 1;
                let view = request.view.as_ref();
                let role = view
                    .map(|view| {
                        if messages_contain(view, REFRESH_TITLE_NEEDLE) {
                            "refresh"
                        } else if messages_contain(view, "ROLE: engineer") {
                            "create"
                        } else {
                            "unknown"
                        }
                    })
                    .unwrap_or("unknown");
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

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
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
