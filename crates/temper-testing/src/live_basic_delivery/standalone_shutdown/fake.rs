use super::*;

#[derive(Default)]
struct ResponseGate {
    state: Mutex<GateState>,
    wake: Condvar,
}

#[derive(Default)]
struct GateState {
    arrived: bool,
    released: bool,
}

impl ResponseGate {
    fn arrive_and_wait(&self) {
        let mut state = self.state.lock().expect("Jig response gate");
        state.arrived = true;
        self.wake.notify_all();
        while !state.released {
            state = self.wake.wait(state).expect("Jig response gate");
        }
    }

    fn wait_for_arrival(&self, timeout: Duration, name: &str) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .state
            .lock()
            .map_err(|_| format!("{name} gate poisoned"))?;
        while !state.arrived {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("timed out waiting for {name} Jig request"));
            }
            let (next, wait) = self
                .wake
                .wait_timeout(state, remaining)
                .map_err(|_| format!("{name} gate poisoned"))?;
            state = next;
            if wait.timed_out() && !state.arrived {
                return Err(format!("timed out waiting for {name} Jig request"));
            }
        }
        Ok(())
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("Jig response gate");
        state.released = true;
        self.wake.notify_all();
    }

    fn released(&self) -> bool {
        self.state.lock().expect("Jig response gate").released
    }
}

pub(super) struct ShutdownFake {
    fake: FakeLlm,
    old_gate: Arc<ResponseGate>,
    replacement_gate: Arc<ResponseGate>,
    replacement_mode: Arc<AtomicBool>,
    replacement_sessions: Arc<AtomicUsize>,
}

impl ShutdownFake {
    pub(super) fn start() -> Result<Self, String> {
        let old_gate = Arc::new(ResponseGate::default());
        let replacement_gate = Arc::new(ResponseGate::default());
        let replacement_mode = Arc::new(AtomicBool::new(false));
        let replacement_started = Arc::new(AtomicBool::new(false));
        let replacement_sessions = Arc::new(AtomicUsize::new(0));
        let rule_old = Arc::clone(&old_gate);
        let rule_replacement = Arc::clone(&replacement_gate);
        let rule_mode = Arc::clone(&replacement_mode);
        let rule_started = Arc::clone(&replacement_started);
        let rule_sessions = Arc::clone(&replacement_sessions);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if !rule_mode.load(Ordering::Acquire) {
                rule_old.arrive_and_wait();
                return Reply::text(
                    json!({
                        "title": "Late old-attempt result",
                        "body": "# Implementation report\nThis result was deliberately released after SIGTERM.",
                        "summary": "A stale result that must remain fenced."
                    })
                    .to_string(),
                );
            }
            if view.prior_tool_results == 0 {
                rule_sessions.fetch_add(1, Ordering::AcqRel);
                if !rule_started.swap(true, Ordering::AcqRel) {
                    rule_replacement.arrive_and_wait();
                }
            }
            replacement_reply(view)
        }))
        .map_err(|error| format!("start gated Jig fake: {error}"))?;
        Ok(Self {
            fake,
            old_gate,
            replacement_gate,
            replacement_mode,
            replacement_sessions,
        })
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn wait_for_old_request(&self, timeout: Duration) -> Result<(), String> {
        self.old_gate.wait_for_arrival(timeout, "old-attempt")
    }

    pub(super) fn release_old_result(&self) {
        self.old_gate.release();
    }

    pub(super) fn old_result_released(&self) -> bool {
        self.old_gate.released()
    }

    pub(super) fn begin_replacement(&self) {
        self.replacement_mode.store(true, Ordering::Release);
    }

    pub(super) fn wait_for_replacement_request(&self, timeout: Duration) -> Result<(), String> {
        self.replacement_gate
            .wait_for_arrival(timeout, "replacement-attempt")
    }

    pub(super) fn release_replacement(&self) {
        self.replacement_gate.release();
    }

    pub(super) fn replacement_sessions(&self) -> usize {
        self.replacement_sessions.load(Ordering::Acquire)
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        if requests.is_empty() {
            return "<no Jig requests>".to_string();
        }
        let start = requests.len().saturating_sub(12);
        requests[start..]
            .iter()
            .enumerate()
            .map(|(offset, request)| {
                let view = request.view.as_ref();
                format!(
                    "#{} {} {} prior_tool_results={}",
                    start + offset + 1,
                    request.method,
                    request.path,
                    view.map_or(0, |view| view.prior_tool_results)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn replacement_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: "write-recovered-standalone".to_string(),
                name: "write".to_string(),
                args: json!({
                    "path": REPLACEMENT_FILE,
                    "content": "replacement standalone recovered exactly once\n"
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: "submit-recovered-standalone".to_string(),
                name: "submit_for_pr".to_string(),
                args: json!({ "summary": REPLACEMENT_SUMMARY }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(
            json!({
                "title": "Recover standalone assignment after bounded shutdown",
                "body": format!("# Implementation report\n{REPLACEMENT_SUMMARY}"),
                "summary": REPLACEMENT_SUMMARY
            })
            .to_string(),
        ),
    }
}
