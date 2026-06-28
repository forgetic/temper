//! Deterministic hermetic agent for the worker e2e — speaks the
//! `smith-agent-protocol` without an LLM or git.
//!
//! Behavior is driven by the worker's spawn flags + a few test env vars:
//! - reads the [`WorkspaceContext`] from the `--context` flag (the new agent
//!   contract — the worker passes paths as flags, not env);
//! - runs in `--workspace` (the prepared scoped workspace root), defaulting to
//!   cwd;
//! - for a writable role: writes `$SMITH_FAKE_AGENT_FILE` (default `GREETING.md`)
//!   with `$SMITH_FAKE_AGENT_CONTENT` into writable repo dirs, so the worker has
//!   a diff to commit/push;
//! - writes a [`WorkspaceResult`] to the `--result` path. If
//!   `$SMITH_FAKE_AGENT_VERDICT` is set, the result carries that verdict (the
//!   read-only / triage path); otherwise it is a head-path result with a summary.
//! - if `--submit-before-result` is passed, calls the worker-owned
//!   `submit_for_pr` side channel before writing the result (and, with
//!   `--submit-retry-after-failure`, calls it again after a fake fix when the
//!   first response is rejected).
//! - if the `--crash-before-result` argument is passed, the process exits
//!   non-zero before writing the result. (An argument, not an env var, so
//!   concurrent test threads cannot race on a process-global knob.)

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;

use temper_protocol_agent::{
    PROTOCOL_VERSION, SUBMIT_FOR_PR_ADDRESS_FLAG, SubmitForPrRequest, SubmitForPrResponse,
    WorkspaceContext, WorkspaceResult,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let context_path = flag_value(&args, "--context").expect("--context flag set");
    let result_path = flag_value(&args, "--result").expect("--result flag set");
    let submit_address = flag_value(&args, SUBMIT_FOR_PR_ADDRESS_FLAG);
    let workspace = flag_value(&args, "--workspace")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let context: WorkspaceContext =
        serde_json::from_slice(&std::fs::read(&context_path).expect("read context"))
            .expect("parse context");

    let verdict = std::env::var("SMITH_FAKE_AGENT_VERDICT").ok();

    // Writable head path: leave a product diff in each writable repo's sibling
    // dir for the worker to commit/push (the cwd is the scoped workspace root;
    // ADR 0023). A single-repo job has exactly one writable repo.
    if verdict.is_none() {
        let file = std::env::var("SMITH_FAKE_AGENT_FILE").unwrap_or_else(|_| "GREETING.md".into());
        let content = std::env::var("SMITH_FAKE_AGENT_CONTENT")
            .unwrap_or_else(|_| "hello from the fake agent\n".into());
        for repo in context.repos.iter().filter(|repo| repo.is_writable()) {
            let repo_dir = workspace.join(&repo.dir);
            std::fs::write(repo_dir.join(&file), content.as_bytes()).expect("write product file");
        }
    }

    if std::env::args().any(|arg| arg == "--crash-before-result") {
        eprintln!("smith-fake-agent: simulated crash before result");
        std::process::exit(7);
    }

    if args.iter().any(|arg| arg == "--submit-before-result") {
        let address = submit_address
            .as_deref()
            .expect("--submit-for-pr-address flag set by runner");
        let first = submit_for_pr(address, &context, Some("fake agent submit"));
        if args.iter().any(|arg| arg == "--submit-retry-after-failure") && !first.accepted {
            for repo in context.repos.iter().filter(|repo| repo.is_writable()) {
                let repo_dir = workspace.join(&repo.dir);
                std::fs::write(
                    repo_dir.join("AFTER_SUBMIT_FAILURE.md"),
                    b"fixed after submit\n",
                )
                .expect("write retry product file");
            }
            let second = submit_for_pr(address, &context, Some("fake agent retry submit"));
            assert!(
                second.accepted,
                "retry submit should be accepted: {second:?}"
            );
        }
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

fn submit_for_pr(
    address: &str,
    context: &WorkspaceContext,
    summary: Option<&str>,
) -> SubmitForPrResponse {
    let request = SubmitForPrRequest {
        protocol_version: PROTOCOL_VERSION,
        correlation_key: context.correlation_key.clone(),
        role: context.work_item.role.clone(),
        action: context.action.clone(),
        summary: summary.map(str::to_string),
    };
    let mut stream = TcpStream::connect(address).expect("connect submit_for_pr side channel");
    let bytes = serde_json::to_vec(&request).expect("serialize submit_for_pr request");
    stream
        .write_all(&bytes)
        .expect("write submit_for_pr request");
    stream
        .shutdown(Shutdown::Write)
        .expect("half-close submit_for_pr request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read submit_for_pr response");
    serde_json::from_slice(&response).expect("parse submit_for_pr response")
}

/// The value following `flag` in `args`, or `None` when the flag is absent.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
