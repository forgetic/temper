use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use temper_protocol_activity::AgentActivityCapturePolicyV1;

use super::tests::context;
use super::*;
use crate::config::WorkerAgentTraceConfig;

const TEST_MAX_RUN_BYTES: u64 = 50_000_000;

fn test_collector(root: &Path) -> TraceCollector {
    TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            max_run_bytes: TEST_MAX_RUN_BYTES,
            ..Default::default()
        },
        spool_root: Some(root.to_path_buf()),
    })
}

fn entry(inventory: &TraceSpoolInventory, outcome: TraceSpoolOutcome) -> &TraceSpoolEntry {
    inventory
        .entries
        .iter()
        .find(|entry| entry.outcome == outcome)
        .expect("classified inventory entry")
}

#[test]
fn inventory_classifies_every_outcome_with_deterministic_saturating_accounting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = test_collector(temp.path());

    let protected = collector
        .begin_run("protected", &context())
        .expect("begin protected")
        .expect("capture enabled");

    let abandoned = collector
        .begin_run("abandoned", &context())
        .expect("begin abandoned")
        .expect("capture enabled");
    drop(abandoned);

    let terminal = collector
        .begin_run("terminal", &context())
        .expect("begin terminal")
        .expect("capture enabled");
    terminal.finish_success(None).expect("finish terminal");
    drop(terminal);

    let compacted = collector
        .begin_run("compacted", &context())
        .expect("begin compacted")
        .expect("capture enabled");
    let terminal_sequence = compacted.finish_success(None).expect("finish compacted");
    compacted
        .acknowledge(terminal_sequence)
        .expect("compact acknowledged terminal");
    drop(compacted);

    let malformed = temp.path().join("malformed-evidence");
    fs::create_dir(&malformed).expect("create malformed directory");
    fs::write(malformed.join("unknown"), b"malformed").expect("write malformed evidence");

    let quarantine = temp.path().join(TRACE_QUARANTINE_DIR);
    fs::create_dir(&quarantine).expect("create quarantine");
    fs::write(quarantine.join("preserved.bad"), b"quarantined")
        .expect("write quarantined evidence");

    let first = collector.inventory().expect("inventory");
    let second = collector.inventory().expect("repeat inventory");
    assert_eq!(first, second, "an unchanged spool has one stable report");
    assert!(
        first
            .entries
            .windows(2)
            .all(|entries| entries[0].path < entries[1].path),
        "entries are sorted by their root-relative path"
    );
    assert_eq!(first.entries.len(), 6);
    assert_eq!(first.outcomes.protected_live_runs, 1);
    assert_eq!(first.outcomes.abandoned_non_terminal_runs, 1);
    assert_eq!(first.outcomes.terminal_unacknowledged_runs, 1);
    assert_eq!(first.outcomes.compacted_runs, 1);
    assert_eq!(first.outcomes.malformed_runs, 1);
    assert_eq!(first.outcomes.quarantined_evidence, 1);
    assert_eq!(first.dirty_run_count, 2);

    let protected_entry = entry(&first, TraceSpoolOutcome::ProtectedLiveRun);
    let abandoned_entry = entry(&first, TraceSpoolOutcome::AbandonedNonTerminalRun);
    let terminal_entry = entry(&first, TraceSpoolOutcome::TerminalUnacknowledgedRun);
    let compacted_entry = entry(&first, TraceSpoolOutcome::CompactedRun);
    let malformed_entry = entry(&first, TraceSpoolOutcome::MalformedRun);
    let quarantined_entry = entry(&first, TraceSpoolOutcome::QuarantinedEvidence);
    assert_eq!(protected_entry.logical_reserved_bytes, TEST_MAX_RUN_BYTES);
    assert_eq!(abandoned_entry.logical_reserved_bytes, TEST_MAX_RUN_BYTES);
    assert_eq!(
        terminal_entry.logical_reserved_bytes, terminal_entry.physical_bytes,
        "immutable terminal runs are charged by actual bytes"
    );
    assert_eq!(
        compacted_entry.logical_reserved_bytes, compacted_entry.physical_bytes,
        "compacted runs are charged by actual bytes"
    );
    assert_eq!(
        malformed_entry.logical_reserved_bytes, malformed_entry.physical_bytes,
        "malformed entries without a valid manifest retain a physical fallback charge"
    );
    assert_eq!(quarantined_entry.logical_reserved_bytes, 0);
    assert_eq!(
        first.total_physical_bytes,
        first.entries.iter().fold(0u64, |total, entry| total
            .saturating_add(entry.physical_bytes))
    );
    assert_eq!(
        first.logical_reserved_bytes,
        first.entries.iter().fold(0u64, |total, entry| total
            .saturating_add(entry.logical_reserved_bytes))
    );
    assert_eq!(
        first.quarantined_physical_bytes,
        quarantined_entry.physical_bytes
    );

    drop(protected);
}

#[test]
fn lifetime_fence_protects_all_clones_without_mutating_live_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = test_collector(temp.path());
    let observer = collector.clone();
    let run = collector
        .begin_run("held-live", &context())
        .expect("begin")
        .expect("capture enabled");
    let last_owner = run.clone();
    let events_path = run.spool_dir().join("events.jsonl");
    let before_events = fs::read(&events_path).expect("read live events");
    let before_sequence = collector.recover().expect("recover live prefix")[0]
        .events
        .last()
        .expect("started event")
        .seq;

    let report = observer.inventory().expect("inventory held run");
    assert_eq!(report.outcomes.protected_live_runs, 1);
    assert_eq!(
        fs::read(&events_path).expect("read unchanged events"),
        before_events
    );
    assert_eq!(
        collector.recover().expect("recover unchanged prefix")[0]
            .events
            .last()
            .expect("started event")
            .seq,
        before_sequence
    );

    drop(run);
    assert_eq!(
        observer
            .inventory()
            .expect("inventory remaining owner")
            .outcomes
            .protected_live_runs,
        1
    );
    drop(last_owner);

    let restarted = test_collector(temp.path());
    let abandoned = restarted.inventory().expect("inventory after restart");
    assert_eq!(abandoned.outcomes.protected_live_runs, 0);
    assert_eq!(abandoned.outcomes.abandoned_non_terminal_runs, 1);
}

#[test]
fn independent_collector_observes_the_advisory_lifetime_fence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let owner = test_collector(temp.path());
    let run = owner
        .begin_run("cross-collector-live", &context())
        .expect("begin")
        .expect("capture enabled");
    let independent = test_collector(temp.path());

    assert_eq!(
        independent
            .inventory()
            .expect("inventory independently held run")
            .outcomes
            .protected_live_runs,
        1
    );
    drop(run);
    assert_eq!(
        independent
            .inventory()
            .expect("inventory released run")
            .outcomes
            .abandoned_non_terminal_runs,
        1
    );
}

#[test]
#[cfg(unix)]
fn clone_shared_active_registry_fences_a_replaced_advisory_lock() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = test_collector(temp.path());
    let observer = collector.clone();
    let run = collector
        .begin_run("local-active", &context())
        .expect("begin")
        .expect("capture enabled");
    fs::remove_file(run.spool_dir().join(".owner.lock"))
        .expect("simulate advisory-lock replacement");

    let report = observer.inventory().expect("inventory clone-shared run");
    assert_eq!(report.outcomes.protected_live_runs, 1);
    assert_eq!(report.outcomes.abandoned_non_terminal_runs, 0);
    drop(run);

    let restarted = test_collector(temp.path());
    assert_eq!(
        restarted
            .inventory()
            .expect("inventory after simulated restart")
            .outcomes
            .abandoned_non_terminal_runs,
        1
    );
}

#[test]
fn malformed_valid_manifest_keeps_its_full_reservation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = test_collector(temp.path());
    let run = collector
        .begin_run("malformed-stream", &context())
        .expect("begin")
        .expect("capture enabled");
    let run_dir = run.spool_dir().to_path_buf();
    drop(run);
    let mut events = OpenOptions::new()
        .append(true)
        .open(run_dir.join("events.jsonl"))
        .expect("open events");
    events.write_all(b"not-json\n").expect("corrupt stream");
    events.sync_all().expect("sync corruption");
    drop(events);

    let report = collector.inventory().expect("inventory malformed stream");
    let malformed = entry(&report, TraceSpoolOutcome::MalformedRun);
    assert_eq!(malformed.logical_reserved_bytes, TEST_MAX_RUN_BYTES);
    assert!(malformed.logical_reserved_bytes >= malformed.physical_bytes);
}

#[test]
fn terminal_actual_byte_accounting_is_the_capacity_admission_authority() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            max_inline_bytes: 1,
            max_blob_bytes: 1,
            max_run_bytes: 5_000,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });

    for index in 0..WORKER_SPOOL_RUN_CAPACITY {
        let run = collector
            .begin_run(&format!("terminal-{index}"), &context())
            .expect("terminal actual bytes leave aggregate capacity")
            .expect("capture enabled");
        run.finish_success(None).expect("finish terminal run");
    }
    let report = collector.inventory().expect("inventory terminal runs");
    assert_eq!(
        report.outcomes.terminal_unacknowledged_runs,
        WORKER_SPOOL_RUN_CAPACITY
    );
    assert!(report.logical_reserved_bytes < 5_000 * WORKER_SPOOL_RUN_CAPACITY);
    collector
        .begin_run("admitted-after-terminal-runs", &context())
        .expect("inventory report authorizes admission")
        .expect("capture enabled");
}

#[test]
#[cfg(unix)]
fn inventory_charges_but_never_traverses_symbolic_links() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    fs::write(outside.path().join("large-evidence"), vec![0u8; 64 * 1024])
        .expect("write external evidence");
    symlink(outside.path(), temp.path().join("unexpected-link")).expect("create symlink");

    let report = test_collector(temp.path())
        .inventory()
        .expect("inventory symlink");
    assert_eq!(report.outcomes.malformed_runs, 1);
    assert_eq!(report.entries.len(), 1);
    assert!(report.total_physical_bytes < 64 * 1024);
    assert_eq!(
        report.logical_reserved_bytes, report.total_physical_bytes,
        "unexpected symlinks receive a conservative metadata charge without traversal"
    );
}
