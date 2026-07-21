// SPDX-License-Identifier: MPL-2.0

use super::*;
use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use temper_protocol_activity::{AgentActivityEventV1, RunFinishedV1, RunStatusV1};
use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};

#[test]
fn authoritative_in_process_cancellation_discards_forge_and_finishes_trace_cancelled() {
    let fake = FakeLlm::start(Script::Fixed(Reply {
        turns: vec![Turn::ToolCall {
            id: "forge-before-fence".to_string(),
            name: "forge_get_item".to_string(),
            args: serde_json::json!({
                "repo": "ai/temper",
                "number": 605,
                "type": "issue",
                "include_comments": false
            }),
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }))
    .expect("start cancellation fake LLM");
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-standalone-cancellation",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());

    let (error, recovered) = temper_engine_io::block_on_with(move |_cx, handle| async move {
        let temp = tempfile::tempdir().expect("standalone cancellation tempdir");
        std::fs::create_dir_all(temp.path().join("temper")).expect("prepared repo dir");
        let spool_root = temp.path().join("spool");
        let host_started = Arc::new(AtomicBool::new(false));
        let host_started_for_call = Arc::clone(&host_started);
        let forge_host: AgentForgeContextHost =
            Arc::new(move |_job_id, _attempt_id, _fence, _operation| {
                host_started_for_call.store(true, Ordering::Release);
                Box::pin(std::future::pending())
            });
        let policy = AgentActivityCapturePolicyV1::default();
        let runner = InProcessAgentRunner::new(handle, provider, 4, None, false)
            .with_forge_context_host(forge_host)
            .with_trace_policy(policy.clone())
            .with_trace_collector(WorkerAgentTraceConfig {
                policy,
                spool_root: Some(spool_root.clone()),
            });
        let context = cancellation_context();
        let fence = temper_worker::AttemptFence::open();
        let cancellation = temper_worker::JobCancellation::default();
        let completed = Arc::new(AtomicBool::new(false));
        let cancel_fence = fence.clone();
        let cancel_handle = cancellation.clone();
        let completed_for_ack = Arc::clone(&completed);
        let acknowledgement_root = spool_root.clone();
        let canceller = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !host_started.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Forge host did not start before cancellation"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            cancel_fence.close();
            cancel_handle.cancel();

            let collector = TraceCollector::new(WorkerAgentTraceConfig {
                policy: AgentActivityCapturePolicyV1::default(),
                spool_root: Some(acknowledgement_root),
            });
            let (run_id, sequence) = loop {
                let runs = collector
                    .recover()
                    .expect("recover pending cancellation trace");
                if let Some((run_id, sequence)) = runs.first().and_then(|run| {
                    run.events.last().and_then(|terminal| {
                        matches!(
                            &terminal.event,
                            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                                status: RunStatusV1::Cancelled,
                                ..
                            })
                        )
                        .then(|| (run.manifest.run_id.clone(), terminal.seq))
                    })
                }) {
                    break (run_id, sequence);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "cancelled terminal was not persisted"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            };
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(
                !completed_for_ack.load(Ordering::Acquire),
                "the historical 250 ms timeout must not release cancellation"
            );
            collector
                .acknowledge(&run_id, sequence)
                .expect("acknowledge cancelled terminal");
        });
        let error = runner
            .run_request(AgentRunRequest::new_controlled(
                "job-standalone-cancel-605",
                "attempt-standalone-cancel-605",
                &context,
                temp.path(),
                fence,
                cancellation,
                temper_worker::JobProgressReporter::noop("attempt-standalone-cancel-605"),
            ))
            .await
            .expect_err("fenced native result must be discarded");
        completed.store(true, Ordering::Release);
        canceller.join().expect("join cancellation trigger");
        let recovered = TraceCollector::new(WorkerAgentTraceConfig {
            policy: AgentActivityCapturePolicyV1::default(),
            spool_root: Some(spool_root),
        })
        .recover()
        .expect("recover standalone cancellation trace");
        (error, recovered)
    });

    assert_eq!(error.class, FailureClass::Canceled);
    assert_eq!(recovered.len(), 1);
    assert!(
        recovered[0].acknowledged_seq > 0,
        "cancelled terminal sequence was durably acknowledged"
    );
    if let Some(terminal) = recovered[0].events.last() {
        assert!(matches!(
            &terminal.event,
            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Cancelled,
                ..
            })
        ));
    }
}

fn cancellation_context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: "forgejo:ai/temper".to_string(),
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            dir: "temper".to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: None,
        }],
        work_item: WorkspaceWorkItem {
            role: "architect".to_string(),
            queue: "intake".to_string(),
            kind: "issue".to_string(),
            target: "Issue { number: ItemNumber(605) }".to_string(),
            context: "{}".to_string(),
        },
        action: "triage_intake".to_string(),
        correlation_key: "attempt-fence-605".to_string(),
        checkout: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}
