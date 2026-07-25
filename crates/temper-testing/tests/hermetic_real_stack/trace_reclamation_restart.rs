use std::time::{Duration, Instant};

use temper_engine::{AgentTraceRun, AgentTraceRunStatus};
use temper_protocol_activity::{AgentActivityCapturePolicyV1, AgentActivityEventV1, CaptureModeV1};
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    FakeModelResponse, HermeticIssueSpec, HermeticRealStack, HermeticRealStackBuilder,
    HermeticSeededTrace, PausePoint,
};
use temper_worker::WORKER_SPOOL_RUN_CAPACITY;

const RECOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const INTERRUPTED_RUNS: usize = WORKER_SPOOL_RUN_CAPACITY as usize;

#[test]
fn replacement_worker_reclaims_saturated_trace_spool_without_losing_evidence() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let policy = AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..AgentActivityCapturePolicyV1::default()
        };
        let mut stack = restart_stack_builder()
            .issue(HermeticIssueSpec::ready_code(
                "Recover trace capacity before assignment",
                "Add RECOVERED_TRACE.md after replacement-worker trace recovery.",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/RECOVERED_TRACE.md",
                "new assignment retained durable activity\n",
                "Recovered trace capacity and completed the new assignment.",
            ))
            .agent_trace_policy(policy.clone())
            .build(&handle)
            .await
            .expect("trace-reclamation real stack builds");

        let mut interrupted = Vec::with_capacity(INTERRUPTED_RUNS);
        for index in 0..INTERRUPTED_RUNS {
            interrupted.push(
                stack
                    .seed_interrupted_agent_trace(
                        &format!("interrupted-trace-{index:02}"),
                        format!("referenced interrupted evidence {index:02}\n").as_bytes(),
                    )
                    .expect("seed complete interrupted trace"),
            );
        }
        stack
            .append_agent_trace_partial_tail(
                &interrupted[5].run_id,
                br#"{"partial_provider_tail":"must be truncated""#,
            )
            .expect("seed incomplete final fragment");
        stack
            .seed_malformed_agent_trace_sibling(
                "zz-corrupt-manifest",
                b"{malformed manifest retained for operator inspection",
            )
            .expect("seed corrupt sibling");

        let aggregate_limit = policy
            .max_run_bytes
            .saturating_mul(WORKER_SPOOL_RUN_CAPACITY);
        let saturated = stack
            .trace_spool_inventory()
            .expect("saturated pre-restart inventory");
        assert_eq!(
            saturated.outcomes.abandoned_non_terminal_runs,
            WORKER_SPOOL_RUN_CAPACITY
        );
        assert_eq!(saturated.outcomes.malformed_runs, 1);
        assert!(saturated.logical_reserved_bytes >= aggregate_limit);
        match stack.seed_live_agent_trace("pre-recovery-admission", b"must not be admitted") {
            Err(error) => assert!(
                error.contains("aggregate activity spool quota exceeded"),
                "pre-feature quota shape must reject another trace: {error}"
            ),
            Ok(_) => panic!("the seventeenth full reservation must be rejected before recovery"),
        }

        // Stop after the daemon has durably accepted one synthetic terminal but
        // before the worker persists its cursor. The next worker must safely
        // retransmit that same batch instead of duplicating the boundary.
        let terminal_ack = stack
            .pause_hooks()
            .arm(PausePoint::WorkerTerminalTraceAcknowledgement);
        stack.start_worker(&handle);
        let terminal_ack = skein::time::timeout(
            temper_engine_io::runtime::timer_now(&cx),
            RECOVERY_TIMEOUT,
            Box::pin(terminal_ack.arrived()),
        )
        .await
        .expect("replacement worker forwards a recovered terminal");
        let accepted_before_cursor = stack
            .trace_runs()
            .expect("journal after first durable terminal acknowledgement");
        assert_eq!(accepted_before_cursor.len(), 1);
        let replayed_run_id = accepted_before_cursor[0].manifest.run_id.clone();
        let durable_terminal_seq = accepted_before_cursor[0]
            .events
            .last()
            .expect("journaled synthetic terminal")
            .seq;
        let pending_payload = stack
            .trace_payload_snapshot(&replayed_run_id)
            .expect("local payload before cursor persistence");
        assert!(!pending_payload.compacted);
        assert!(!pending_payload.events.is_empty());
        assert!(!pending_payload.blobs.is_empty());
        assert!(pending_payload.acknowledged_sequence < durable_terminal_seq);

        stack.crash_worker().await;
        drop(terminal_ack);
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());

        // The second replacement runs the same production startup path. Enqueue
        // immediately: admission depends on terminal-run physical accounting,
        // not on waiting for all journal acknowledgements and compaction.
        stack.start_worker(&handle);
        let assignment = match stack.run_open_pr_job(&cx, &handle).await {
            Ok(assignment) => assignment,
            Err(error) => panic!(
                "new assignment did not run after startup reclamation: {error}; active={:?}; published={:?}; inventory={:?}; journal={:?}; assignment_pauses={}",
                stack
                    .active_worker_tasks()
                    .iter()
                    .map(|task| (task.job_id(), task.join_state()))
                    .collect::<Vec<_>>(),
                stack.published_results(),
                stack.trace_spool_inventory(),
                stack.trace_runs().map(|runs| runs
                    .iter()
                    .map(|run| (
                        run.manifest.assignment.job_id.clone(),
                        run.summary.status,
                        run.summary.last_accepted_seq,
                    ))
                    .collect::<Vec<_>>()),
                stack
                    .pause_hooks()
                    .reached_count(PausePoint::AssignmentClaimCommitted),
            ),
        };
        assert_eq!(assignment.job_result.status, ResultStatus::Success);
        assert_eq!(assignment.pull_requests.len(), 1);

        let recovered = wait_for_seeded_recovery(&stack, &cx, &interrupted).await;
        for seed in &interrupted {
            assert_seeded_journal(&recovered, seed);
            let local = stack
                .trace_payload_snapshot(&seed.run_id)
                .expect("compacted interrupted spool");
            assert!(local.compacted);
            assert!(local.events.is_empty());
            assert!(local.blobs.is_empty());
        }
        assert_seeded_journal(&recovered, &interrupted[0]);
        let retransmitted = recovered
            .iter()
            .find(|run| run.manifest.run_id == replayed_run_id)
            .expect("retransmitted run remains journaled once");
        assert_eq!(
            retransmitted
                .events
                .iter()
                .filter(|event| event.event.is_terminal())
                .count(),
            1,
            "retransmission must not duplicate the synthetic terminal"
        );

        let successful =
            wait_for_successful_assignment_trace(&stack, &cx, &assignment.job_result.job_id).await;
        assert_eq!(successful.manifest.capture_policy, policy);
        assert!(successful.events.iter().any(|event| matches!(
            event.event,
            AgentActivityEventV1::ModelCallStarted(_) | AgentActivityEventV1::ToolStarted(_)
        )));
        assert_eq!(
            successful
                .events
                .iter()
                .filter(|event| event.event.is_terminal())
                .count(),
            1
        );

        let quarantined = stack
            .trace_spool_inventory()
            .expect("post-recovery inventory");
        assert_eq!(quarantined.outcomes.malformed_runs, 0);
        assert_eq!(quarantined.outcomes.quarantined_evidence, 1);
        assert!(quarantined.quarantined_physical_bytes > 0);

        // A live owner is inspected by a later startup pass but its complete
        // payload and sequence cannot change. Once ownership ends, the bounded
        // background continuation terminalizes, forwards, and compacts it.
        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        let live = stack
            .seed_live_agent_trace(
                "live-protected-recovery",
                b"live referenced evidence remains byte-for-byte stable",
            )
            .expect("seed protected live trace");
        let live_evidence = live.evidence().clone();
        let live_before = stack
            .trace_payload_snapshot(live.run_id())
            .expect("snapshot live trace before recovery");
        stack.start_worker(&handle);
        let live_after_startup = stack
            .trace_payload_snapshot(live.run_id())
            .expect("snapshot live trace after startup pass");
        assert_eq!(live_after_startup, live_before);
        let protected_inventory = stack
            .trace_spool_inventory()
            .expect("protected startup inventory");
        assert_eq!(protected_inventory.outcomes.protected_live_runs, 1);
        drop(live);

        let live_journal = wait_for_one_seeded_recovery(&stack, &cx, &live_evidence).await;
        assert_seeded_run(&live_journal, &live_evidence);
        let live_compacted = stack
            .trace_payload_snapshot(&live_evidence.run_id)
            .expect("released live trace compacted after acknowledgement");
        assert!(live_compacted.compacted);
        assert!(live_compacted.events.is_empty());
        assert!(live_compacted.blobs.is_empty());

        // Repeated startup passes are no-ops for terminal count and logical
        // accounting. Quarantined bytes remain visible but consume no logical
        // active-spool reservation.
        let stable_inventory = stack
            .trace_spool_inventory()
            .expect("stable converged inventory");
        assert_eq!(stable_inventory.dirty_run_count, 0);
        assert_eq!(stable_inventory.outcomes.malformed_runs, 0);
        stack.crash_worker().await;
        stack.replace_daemon(&handle).await;
        assert!(stack.open_recovery_barrier().await.is_empty());
        stack.start_worker(&handle);
        assert_eq!(
            stack
                .trace_spool_inventory()
                .expect("immediate repeated-start inventory"),
            stable_inventory
        );
        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(250)).await;
        assert_eq!(
            stack
                .trace_spool_inventory()
                .expect("post-background repeated-start inventory"),
            stable_inventory
        );
        let final_runs = stack.trace_runs().expect("final journal inventory");
        for seed in interrupted.iter().chain(std::iter::once(&live_evidence)) {
            assert_seeded_journal(&final_runs, seed);
        }
        stack.crash_worker().await;
    });
}

fn restart_stack_builder() -> HermeticRealStackBuilder {
    let builder = HermeticRealStackBuilder::new();
    #[cfg(target_os = "linux")]
    let builder =
        builder.linux_supervisor_helper(env!("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"));
    builder
}

async fn wait_for_seeded_recovery(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    seeds: &[HermeticSeededTrace],
) -> Vec<AgentTraceRun> {
    let deadline = Instant::now() + RECOVERY_TIMEOUT;
    loop {
        let runs = stack.trace_runs().expect("read recovery journal");
        let journal_complete = seeds.iter().all(|seed| {
            runs.iter().any(|run| {
                run.manifest.run_id == seed.run_id
                    && run.summary.status == AgentTraceRunStatus::Failed
                    && run
                        .events
                        .last()
                        .is_some_and(|event| event.event.is_terminal())
            })
        });
        let local_compacted = seeds.iter().all(|seed| {
            stack
                .trace_payload_snapshot(&seed.run_id)
                .is_ok_and(|snapshot| snapshot.compacted)
        });
        let inventory = stack
            .trace_spool_inventory()
            .expect("read recovery spool inventory");
        if journal_complete
            && local_compacted
            && inventory.outcomes.malformed_runs == 0
            && inventory.outcomes.quarantined_evidence == 1
        {
            return runs;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {} seeded traces: journal={} compacted={} inventory={inventory:?}",
            seeds.len(),
            runs.len(),
            local_compacted,
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn wait_for_one_seeded_recovery(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    seed: &HermeticSeededTrace,
) -> AgentTraceRun {
    let deadline = Instant::now() + RECOVERY_TIMEOUT;
    loop {
        let matching = stack
            .trace_runs()
            .expect("read protected-run journal")
            .into_iter()
            .find(|run| {
                run.manifest.run_id == seed.run_id
                    && run.summary.status == AgentTraceRunStatus::Failed
            });
        if let Some(run) = matching {
            if stack
                .trace_payload_snapshot(&seed.run_id)
                .is_ok_and(|snapshot| snapshot.compacted)
            {
                return run;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for released live trace {}",
            seed.run_id
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn wait_for_successful_assignment_trace(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    job_id: &str,
) -> AgentTraceRun {
    let deadline = Instant::now() + RECOVERY_TIMEOUT;
    loop {
        if let Some(run) = stack
            .trace_runs()
            .expect("read assignment trace journal")
            .into_iter()
            .find(|run| {
                run.manifest.assignment.job_id == job_id
                    && run.summary.status == AgentTraceRunStatus::Succeeded
            })
        {
            if stack
                .trace_payload_snapshot(&run.manifest.run_id)
                .is_ok_and(|snapshot| snapshot.compacted)
            {
                return run;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for successful durable trace for assignment {job_id}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

fn assert_seeded_journal(runs: &[AgentTraceRun], seed: &HermeticSeededTrace) {
    let run = runs
        .iter()
        .find(|run| run.manifest.run_id == seed.run_id)
        .unwrap_or_else(|| panic!("journal is missing seeded run {}", seed.run_id));
    assert_seeded_run(run, seed);
}

fn assert_seeded_run(run: &AgentTraceRun, seed: &HermeticSeededTrace) {
    assert_eq!(run.summary.status, AgentTraceRunStatus::Failed);
    assert_eq!(
        &run.events[..seed.events.len()],
        seed.events.as_slice(),
        "every complete pre-restart event must remain exact"
    );
    assert_eq!(run.attachments, seed.blobs);
    assert_eq!(run.events.len(), seed.events.len() + 1);
    assert_eq!(
        run.events
            .iter()
            .filter(|event| event.event.is_terminal())
            .count(),
        1,
        "one abandoned run receives exactly one synthetic terminal"
    );
}
