use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use temper_protocol_activity::{ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentRunEventV1};

use super::{
    RecoveredTraceRun, TraceCollector, TraceError, acknowledge_recovered_run,
    event_blob_references, read_dir, recover_run, set_private_dir,
};

impl RecoveredTraceRun {
    /// Builds the next at-least-once forwarding batch after the durable cursor.
    /// Blob attachments are included exactly when selected events reference them.
    pub fn pending_batch(&self, max_events: usize) -> Option<AgentActivityBatch> {
        self.pending_batch_bounded(max_events, usize::MAX)
    }

    /// Builds a count- and encoded-byte-bounded forwarding batch. The byte
    /// limit is soft only for a single event plus its required blob, ensuring a
    /// legal large record can always make progress instead of wedging the run.
    pub fn pending_batch_bounded(
        &self,
        max_events: usize,
        max_encoded_bytes: usize,
    ) -> Option<AgentActivityBatch> {
        if max_events == 0 || max_encoded_bytes == 0 {
            return None;
        }
        let pending = self
            .events
            .iter()
            .filter(|event| event.seq > self.acknowledged_seq);
        let mut selected = Vec::new();
        for event in pending.take(max_events) {
            selected.push(event.clone());
            let candidate = self.batch_for_events(&selected)?;
            if selected.len() > 1
                && serde_json::to_vec(&candidate)
                    .map_or(true, |encoded| encoded.len() > max_encoded_bytes)
            {
                selected.pop();
                break;
            }
        }
        self.batch_for_events(&selected)
    }

    fn batch_for_events(&self, events: &[AgentRunEventV1]) -> Option<AgentActivityBatch> {
        let first_seq = events.first()?.seq;
        let referenced = events
            .iter()
            .flat_map(|event| event_blob_references(&event.event))
            .map(|reference| reference.digest.as_str())
            .collect::<BTreeSet<_>>();
        let blobs = self
            .blobs
            .iter()
            .filter(|attachment| referenced.contains(attachment.blob.digest.as_str()))
            .cloned()
            .collect();
        Some(AgentActivityBatch {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.manifest.run_id.clone(),
            first_seq,
            events: events.to_vec(),
            blobs,
        })
    }
}

impl TraceCollector {
    /// Recovers every independently readable run while quarantining corrupt
    /// siblings from the forwarding pass. A single damaged spool must not
    /// prevent unrelated jobs' traces from draining.
    pub(super) fn recover_forwardable(&self) -> Result<Vec<RecoveredTraceRun>, TraceError> {
        let Some(root) = self.config.spool_root.as_deref() else {
            return Ok(Vec::new());
        };
        if !root.exists() {
            return Ok(Vec::new());
        }
        set_private_dir(root)?;
        let mut run_dirs = read_dir(root)?
            .filter_map(Result::ok)
            .filter_map(|entry| match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => Some(entry.path()),
                _ => None,
            })
            .collect::<Vec<_>>();
        run_dirs.sort();
        let mut runs = Vec::new();
        for run_dir in run_dirs {
            match recover_run(&run_dir) {
                Ok(run) => runs.push(run),
                Err(error) => tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.spool_skipped",
                    spool = %run_dir.display(),
                    %error,
                    "worker skipped a corrupt activity spool and continued forwarding others"
                ),
            }
        }
        Ok(runs)
    }

    /// Advances one recovered run's durable forwarding cursor. The complete
    /// spool is retained for crash diagnosis; the cursor is the compaction
    /// boundary and never advances beyond records verified on disk.
    pub fn acknowledge(&self, run_id: &str, highest_contiguous_seq: u64) -> Result<(), TraceError> {
        let root = self
            .config
            .spool_root
            .as_deref()
            .ok_or(TraceError::Disabled)?;
        acknowledge_recovered_run(&root.join(run_id), highest_contiguous_seq)
    }

    /// Gives the independent forwarder a bounded opportunity to durably flush
    /// a terminal cursor. Timeout/storage failures return `false`; callers must
    /// always preserve the original agent/job outcome.
    pub async fn await_acknowledged(&self, run_id: &str, sequence: u64, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            if self.recover_forwardable().ok().is_some_and(|runs| {
                runs.into_iter()
                    .any(|run| run.manifest.run_id == run_id && run.acknowledged_seq >= sequence)
            }) {
                return true;
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return false;
            }
            temper_worker_io::sleep_for(
                Duration::from_millis(25).min(timeout.saturating_sub(elapsed)),
            )
            .await;
        }
    }
}
