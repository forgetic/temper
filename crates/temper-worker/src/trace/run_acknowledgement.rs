use super::*;

impl TraceRun {
    /// Atomically advances the durable forwarding cursor. Retransmitted or
    /// lower acknowledgements are idempotent; a cursor beyond durable data is rejected.
    pub fn acknowledge(&self, highest_contiguous_seq: u64) -> Result<(), TraceError> {
        let mut state = self.inner.state.lock().expect("trace run state lock");
        if state.disabled {
            return Err(TraceError::Disabled);
        }
        let last_seq = state.next_seq.saturating_sub(1);
        if highest_contiguous_seq > last_seq {
            return Err(TraceError::InvalidAcknowledgement {
                acknowledged: highest_contiguous_seq,
                last_seq,
            });
        }
        if highest_contiguous_seq <= state.acknowledged_seq {
            return Ok(());
        }
        let event_end_offset = state
            .event_end_offsets
            .get(usize::try_from(highest_contiguous_seq.saturating_sub(1)).unwrap_or(usize::MAX))
            .copied()
            .ok_or_else(|| {
                TraceError::InvalidSpool(format!(
                    "run {} has no event boundary for acknowledgement {}",
                    self.inner.manifest.run_id, highest_contiguous_seq
                ))
            })?;
        let compact_terminal = state.terminal && highest_contiguous_seq == last_seq;
        let cursor = TraceAckCursorV1::new(&self.inner.manifest.run_id, highest_contiguous_seq);
        let bytes = serde_json::to_vec_pretty(&cursor)?;
        lock_spool(&self.inner)?;
        if let Err(error) = atomic_write(&self.inner.cursor_path, &bytes, true) {
            let _ = unlock_spool(&self.inner);
            state.disabled = true;
            return Err(error);
        }
        if !compact_terminal {
            if let Err(error) = persist_forwarding_index(
                &self.inner.run_dir,
                &self.inner.manifest.run_id,
                highest_contiguous_seq,
                event_end_offset,
            ) {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.forwarding_index_discarded",
                    spool = %self.inner.run_dir.display(),
                    %error,
                    "worker could not persist discardable activity forwarding metadata"
                );
            }
        }
        unlock_spool(&self.inner)?;
        state.acknowledged_seq = highest_contiguous_seq;
        drop(state);
        self.inner.coordination.publish_acknowledgement();
        if compact_terminal {
            acknowledge_recovered_run(&self.inner.run_dir, highest_contiguous_seq)?;
        }
        Ok(())
    }
}
