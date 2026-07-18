use std::collections::BTreeSet;
use std::future::Future;
use std::task::Poll;
use std::time::{Duration, Instant};

use temper_protocol_activity::{ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentRunEventV1};

use super::{
    RecoveredTraceRun, TraceCollector, TraceError, acknowledge_recovered_run,
    event_blob_references, read_acknowledged_sequence, read_dir, recover_run, set_private_dir,
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

    /// Advances one recovered run's durable forwarding cursor. Partial
    /// acknowledgements retain the complete restart-readable spool. Once the
    /// terminal sequence is acknowledged, the event/blob payload is replaced
    /// by a compact durable acknowledgement marker.
    pub fn acknowledge(&self, run_id: &str, highest_contiguous_seq: u64) -> Result<(), TraceError> {
        let root = self
            .config
            .spool_root
            .as_deref()
            .ok_or(TraceError::Disabled)?;
        let advanced = acknowledge_recovered_run(&root.join(run_id), highest_contiguous_seq)?;
        if advanced {
            self.coordination.publish_acknowledgement();
        }
        Ok(())
    }

    /// Gives the independent forwarder a bounded opportunity to durably flush
    /// a terminal cursor. Timeout/storage failures return `false`; callers must
    /// always preserve the original agent/job outcome.
    pub async fn await_acknowledged(&self, run_id: &str, sequence: u64, timeout: Duration) -> bool {
        let Some(root) = self.config.spool_root.as_deref() else {
            return false;
        };
        let run_dir = root.join(run_id);
        let started = Instant::now();
        loop {
            // Snapshot before reading the cursor. An acknowledgement published
            // between this read and waiter registration necessarily advances
            // the generation, making the wait immediately ready.
            let generation = self.coordination_snapshot().acknowledgement_generation;
            if read_acknowledged_sequence(&run_dir, run_id)
                .is_ok_and(|acknowledged| acknowledged >= sequence)
            {
                return true;
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return false;
            }
            let remaining = timeout.saturating_sub(elapsed);
            let mut changed = std::pin::pin!(self.wait_for_acknowledgement(generation));
            let mut timed_out = std::pin::pin!(temper_worker_io::sleep_for(remaining));
            let acknowledgement_changed = std::future::poll_fn(|cx| {
                if changed.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(true);
                }
                if timed_out.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(false);
                }
                Poll::Pending
            })
            .await;
            if !acknowledgement_changed {
                // Config-based compatibility composition may still construct
                // a separate collector for the forwarder. One final targeted
                // cursor read preserves its bounded flush behavior without
                // returning to full-spool polling.
                return read_acknowledged_sequence(&run_dir, run_id)
                    .is_ok_and(|acknowledged| acknowledged >= sequence);
            }
        }
    }
}
