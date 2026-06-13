//! Deterministic hermetic agent for the worker e2e — speaks the
//! `smith-agent-protocol` without an LLM or git.
//!
//! Behavior is driven by env vars the test sets:
//! - reads the [`WorkspaceContext`] from `$TEMPER_CODING_WORKSPACE_CONTEXT`;
//! - for a writable role: writes `$SMITH_FAKE_AGENT_FILE` (default `GREETING.md`)
//!   with `$SMITH_FAKE_AGENT_CONTENT` into the cwd (the prepared checkout), so
//!   the worker has a diff to commit/push;
//! - emits two step-progress lines on stdout (a `Started` and a `Done` marker),
//!   both stamped with the context's `correlation_key`;
//! - writes a [`WorkspaceResult`] to `$TEMPER_CODING_WORKSPACE_RESULT`. If
//!   `$SMITH_FAKE_AGENT_VERDICT` is set, the result carries that verdict (the
//!   read-only / triage path); otherwise it is a head-path result with a summary.
//! - if the `--crash-after-progress` argument is passed, the process exits
//!   non-zero *after* emitting progress but *before* writing the result — the
//!   crash-recovery scenario. (An argument, not an env var, so concurrent test
//!   threads cannot race on a process-global knob.)

use std::io::Write;

use smith_agent_protocol::{
    CONTEXT_ENV, RESULT_ENV, StepProgress, StepState, WorkspaceContext, WorkspaceResult,
};

fn main() {
    let context_path = std::env::var(CONTEXT_ENV).expect("CONTEXT_ENV set");
    let result_path = std::env::var(RESULT_ENV).expect("RESULT_ENV set");
    let context: WorkspaceContext =
        serde_json::from_slice(&std::fs::read(&context_path).expect("read context"))
            .expect("parse context");

    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: 1,
        status: format!("start {} run", context.work_item.role),
        state: StepState::Started,
        pushed_sha: None,
        note: None,
    });

    let verdict = std::env::var("SMITH_FAKE_AGENT_VERDICT").ok();

    // Writable head path: leave a product diff for the worker to commit/push.
    if verdict.is_none() {
        let file = std::env::var("SMITH_FAKE_AGENT_FILE").unwrap_or_else(|_| "GREETING.md".into());
        let content = std::env::var("SMITH_FAKE_AGENT_CONTENT")
            .unwrap_or_else(|_| "hello from the fake agent\n".into());
        let cwd = std::env::current_dir().expect("cwd");
        std::fs::write(cwd.join(&file), content).expect("write product file");
    }

    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: 2,
        status: "produce work product".to_string(),
        state: StepState::Done,
        pushed_sha: None,
        note: Some("fake agent done".to_string()),
    });

    if std::env::args().any(|arg| arg == "--crash-after-progress") {
        eprintln!("smith-fake-agent: simulated crash after progress");
        std::process::exit(7);
    }

    let result = if let Some(verdict) = verdict {
        WorkspaceResult {
            verdict: Some(verdict),
            summary: Some("fake triage".to_string()),
            body: std::env::var("SMITH_FAKE_AGENT_BODY").ok(),
            ..Default::default()
        }
    } else {
        WorkspaceResult {
            summary: Some("fake agent created the product file".to_string()),
            ..Default::default()
        }
    };

    let bytes = serde_json::to_vec_pretty(&result).expect("serialize result");
    std::fs::write(&result_path, bytes).expect("write result");
}

fn emit(progress: &StepProgress) {
    let line = progress.to_line().expect("serialize progress");
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").expect("write progress line");
    stdout.flush().expect("flush");
}
