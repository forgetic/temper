use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use fs2::FileExt as _;
use serde_json::Value as JsonValue;
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, AgentTerminalReasonV1, RunFinishedV1,
    RunStatusV1, ScopeFinishedV1, ScopeStatusV1,
};
use temper_worker::{TraceCollector, WorkerAgentTraceConfig};

pub(super) struct TraceJournalObstruction(std::fs::File);

impl TraceJournalObstruction {
    pub(super) fn acquire(journal_root: &Path, timeout: Duration) -> Result<Self, String> {
        let lock_path = journal_root.join(".journal.lock");
        let deadline = Instant::now() + timeout;
        while !lock_path.is_file() {
            if Instant::now() >= deadline {
                return Err(format!(
                    "timed out waiting for trace journal lock {}",
                    lock_path.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| format!("open trace journal lock {}: {error}", lock_path.display()))?;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self(file)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "timed out acquiring trace journal obstruction {}",
                            lock_path.display()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(format!(
                        "obstruct trace journal {}: {error}",
                        lock_path.display()
                    ));
                }
            }
        }
    }

    pub(super) fn release(self) -> Result<(), String> {
        fs2::FileExt::unlock(&self.0)
            .map_err(|error| format!("release trace journal obstruction: {error}"))
    }
}

pub(super) struct OldTraceEvidence {
    pub(super) run_id: String,
    pub(super) last_sequence: u64,
    pub(super) pending: bool,
}

pub(super) fn old_trace_evidence(
    spool_root: &Path,
    job_id: &str,
) -> Result<OldTraceEvidence, String> {
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(spool_root.to_path_buf()),
    });
    let mut matching = collector
        .recover()
        .map_err(|error| format!("recover prior standalone trace spool: {error}"))?
        .into_iter()
        .filter(|run| run.manifest.assignment.job_id == job_id)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(format!(
            "expected one prior-attempt trace run for {job_id}, found {}",
            matching.len()
        ));
    }
    let run = matching.pop().expect("one trace run");
    let terminal = run
        .events
        .last()
        .ok_or_else(|| "prior attempt trace contained no durable events".to_string())?;
    let last_sequence = terminal.seq;
    let cancelled_boundary = matches!(
        &terminal.event,
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Cancelled,
            ..
        }) | AgentActivityEventV1::ScopeFinished(ScopeFinishedV1 {
            status: ScopeStatusV1::Cancelled,
            terminal_reason: Some(AgentTerminalReasonV1::Aborted),
            ..
        })
    );
    if !cancelled_boundary {
        return Err(format!(
            "prior attempt trace {} did not retain a cancelled terminal boundary: {:?}",
            run.manifest.run_id, terminal.event
        ));
    }
    if run.acknowledged_seq >= last_sequence {
        return Err(format!(
            "prior attempt trace {} was already acknowledged through terminal sequence {last_sequence}; the restart path was not exercised",
            run.manifest.run_id
        ));
    }
    Ok(OldTraceEvidence {
        run_id: run.manifest.run_id,
        last_sequence,
        pending: true,
    })
}

pub(super) fn wait_for_journal_sequence(
    journal_root: &Path,
    run_id: &str,
    expected: u64,
    timeout: Duration,
) -> Result<u64, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(sequence) = journal_sequence(journal_root, run_id)? {
            if sequence >= expected {
                return Ok(sequence);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "restarted standalone did not forward trace run {run_id} through sequence {expected}"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn journal_sequence(root: &Path, run_id: &str) -> Result<Option<u64>, String> {
    let runs = root.join("runs");
    let entries = match fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read trace journal {}: {error}", runs.display())),
    };
    for entry in entries.flatten() {
        let manifest_path = entry.path().join("manifest.json");
        let summary_path = entry.path().join("summary.json");
        let Ok(manifest) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<JsonValue>(&manifest) else {
            continue;
        };
        if manifest.get("run_id").and_then(JsonValue::as_str) != Some(run_id) {
            continue;
        }
        let summary = fs::read_to_string(&summary_path)
            .map_err(|error| format!("read trace summary {}: {error}", summary_path.display()))?;
        let summary: JsonValue = serde_json::from_str(&summary)
            .map_err(|error| format!("parse trace summary {}: {error}", summary_path.display()))?;
        return Ok(summary.get("last_accepted_seq").and_then(JsonValue::as_u64));
    }
    Ok(None)
}
