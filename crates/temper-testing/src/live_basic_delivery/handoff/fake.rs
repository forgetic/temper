use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::json;

use super::{HandoffFixture, REFRESH_FAKE_TIMEOUT};

const REFRESH_TITLE_NEEDLE: &str = "refresh existing handoff";
const CREATE_NOTE_PATH: &str = "service/HANDOFF_CREATE.md";
const REFRESH_NOTE_PATH: &str = "service/HANDOFF_REFRESH.md";

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
    pub(super) fn start(fixture: HandoffFixture) -> Self {
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
            handoff_reply(&fixture, refresh, view.prior_tool_results)
        }))
        .expect("start implementation-pr-handoff fake LLM");
        Self {
            fake,
            state,
            engineer_requests,
        }
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

fn handoff_reply(fixture: &HandoffFixture, refresh: bool, prior_tool_results: usize) -> Reply {
    match prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: if refresh {
                    "call_write_refresh_handoff_note".to_string()
                } else {
                    "call_write_create_handoff_note".to_string()
                },
                name: "write".to_string(),
                args: json!({
                    "path": if refresh { REFRESH_NOTE_PATH } else { CREATE_NOTE_PATH },
                    "content": if refresh {
                        "refresh handoff product diff\n"
                    } else {
                        "create handoff product diff\n"
                    },
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: if refresh {
                    "call_submit_refresh_handoff".to_string()
                } else {
                    "call_submit_create_handoff".to_string()
                },
                name: "submit_for_pr".to_string(),
                args: json!({ "summary": fixture.summary }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => {
            let (title, body) = if refresh {
                (&fixture.refresh_title, &fixture.refresh_body)
            } else {
                (&fixture.create_title, &fixture.create_body)
            };
            Reply::text(
                json!({
                    "title": title,
                    "body": body,
                    "summary": fixture.summary,
                })
                .to_string(),
            )
        }
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
