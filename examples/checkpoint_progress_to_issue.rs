//! Replay agent step-progress markers through the daemon's progress applier and
//! print the issue's ticked checklist.
//!
//! Reads a JSONL file of `smith-agent-protocol` `StepProgress` records (the
//! exact lines an agent prints on stdout — `argv[1]`), then for each one calls
//! the **real** [`temper_daemon::ForgeApplier::apply_progress`] against a fresh
//! in-memory forge issue. The output is the human-facing checklist the daemon
//! posts on the coordinating issue/PR as the agent checkpoints.
//!
//! Used by `smith/examples/checkpoint-live` to show the second half of the live
//! flow (the first half being a real agent calling the `checkpoint` tool).

use std::sync::Arc;

use temper_daemon::{ForgeApplier, InFlightJob, ResultApplier};
use temper_forge::{CreateIssue, CreateRepository, Forge, UserId};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{Artifact, JobProgress, WORKER_PROTOCOL_VERSION};
use temper_workflow::RawWorkflowSpec;

const WORKFLOW: &str = include_str!("../crates/temper-workflow/fixtures/basic-delivery.json");

/// In-memory forge futures resolve without parking, so a manual poll is enough.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => panic!("in-memory forge futures should not park"),
        }
    }
}

fn main() {
    let markers_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: checkpoint_progress_to_issue <markers.jsonl>");
        std::process::exit(2);
    });
    let markers = std::fs::read_to_string(&markers_path)
        .unwrap_or_else(|error| panic!("read markers {markers_path}: {error}"));

    let forge = Arc::new(MemoryForge::new());
    let repo = block_on(forge.create_repository(CreateRepository {
        owner: "ai".into(),
        name: "demo".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("create repo")
    .id;
    let issue = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Coordinating issue for the live checkpoint run".into(),
            body: "The engineer's progress is recorded here as it checkpoints.".into(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("create issue");

    let workflow = Arc::new(
        serde_json::from_str::<RawWorkflowSpec>(WORKFLOW)
            .expect("workflow parses")
            .validate()
            .expect("workflow validates"),
    );
    let applier = ForgeApplier::new(forge.clone(), workflow);

    // The coordinating-issue job the progress is keyed to (the worker resolves
    // it by correlation key in production; here we build it directly).
    let job = InFlightJob {
        job_id: "ai/demo/issue-1/engineer/code_ready".into(),
        role: "engineer".into(),
        repo: "ai/demo".into(),
        artifact: Artifact {
            item: serde_json::json!(issue.number.get()),
            kind: "issue".into(),
        },
        job_payload: serde_json::json!({}),
    };

    let mut applied = 0usize;
    for line in markers.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|error| panic!("marker json: {error}"));
        // Only step-progress records carry these fields; skip anything else.
        let (Some(correlation_key), Some(step), Some(status), Some(state)) = (
            value.get("correlation_key").and_then(|v| v.as_str()),
            value.get("step").and_then(|v| v.as_u64()),
            value.get("status").and_then(|v| v.as_str()),
            value.get("state").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let progress = JobProgress {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "smith-worker-live".into(),
            correlation_key: correlation_key.to_string(),
            step: step as u32,
            status: status.to_string(),
            state: state.to_string(),
            pushed_sha: value
                .get("pushed_sha")
                .and_then(|v| v.as_str())
                .map(String::from),
            note: value.get("note").and_then(|v| v.as_str()).map(String::from),
        };
        block_on(applier.apply_progress(job.clone(), progress));
        applied += 1;
    }

    let comments = block_on(forge.list_issue_comments(&issue.id)).expect("list comments");
    println!(
        "issue #{}: {applied} progress markers applied -> {} checklist comment(s):",
        issue.number.get(),
        comments.len()
    );
    println!("---------------------------------------------------------------");
    for comment in &comments {
        for checklist_line in comment.body.lines().filter(|line| line.starts_with("- [")) {
            println!("{checklist_line}");
        }
    }
    println!("---------------------------------------------------------------");

    // Prove the checkpoint markers ticked the list.
    let ticked = comments
        .iter()
        .filter(|comment| comment.body.contains("- [x] step"))
        .count();
    assert!(
        ticked >= 1,
        "expected at least one ticked checkpoint on the issue"
    );
}
