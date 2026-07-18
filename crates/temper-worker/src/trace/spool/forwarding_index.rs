use super::*;

pub(in crate::trace) fn persist_forwarding_index(
    run_dir: &Path,
    run_id: &str,
    highest_contiguous_seq: u64,
    event_end_offset: u64,
) -> Result<(), TraceError> {
    let index = TraceForwardingIndexV1 {
        version: FORWARDING_INDEX_VERSION,
        run_id: run_id.to_string(),
        highest_contiguous_seq,
        event_end_offset,
    };
    let bytes = serde_json::to_vec_pretty(&index)?;
    atomic_write(&run_dir.join(FORWARDING_INDEX_FILE), &bytes, true)
}

pub(super) fn persist_forwarding_index_best_effort(
    run_dir: &Path,
    run_id: &str,
    highest_contiguous_seq: u64,
    event_end_offset: u64,
) {
    if let Err(error) =
        persist_forwarding_index(run_dir, run_id, highest_contiguous_seq, event_end_offset)
    {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "agent.activity.forwarding_index_discarded",
            spool = %run_dir.display(),
            %error,
            "worker could not persist discardable activity forwarding metadata"
        );
    }
}

pub(in crate::trace) fn acknowledge_forwarded_run(
    run_dir: &Path,
    boundary: ForwardingAcknowledgementBoundary,
) -> Result<bool, TraceError> {
    acknowledge_run_locked_at_root(run_dir, |run_dir| {
        let metadata = recover_spool_metadata(run_dir)?;
        if recover_compacted_marker(run_dir, &metadata)? {
            return if boundary.sequence <= metadata.cursor.highest_contiguous_seq {
                Ok(false)
            } else {
                Err(TraceError::InvalidAcknowledgement {
                    acknowledged: boundary.sequence,
                    last_seq: metadata.cursor.highest_contiguous_seq,
                })
            };
        }
        if boundary.sequence <= metadata.cursor.highest_contiguous_seq {
            return Ok(false);
        }
        let events_path = run_dir.join("events.jsonl");
        let event_file_len = fs::metadata(&events_path)
            .map_err(|source| io_error("inspect activity records", &events_path, source))?
            .len();
        if boundary.sequence == 0
            || boundary.event_end_offset == 0
            || boundary.event_end_offset > event_file_len
            || (boundary.terminal && boundary.event_end_offset != event_file_len)
        {
            return Err(TraceError::InvalidSpool(format!(
                "run {} has an invalid forwarded acknowledgement boundary",
                metadata.manifest.run_id
            )));
        }

        // A crash hereafter can leave a newer cursor with an older or missing
        // index. Forwarding recovery detects that mismatch and safely performs
        // full recovery rather than skipping any payload.
        write_acknowledgement_cursor(run_dir, &metadata.manifest.run_id, boundary.sequence)?;
        if boundary.terminal {
            compact_fully_acknowledged_run(run_dir, &metadata.manifest.run_id, boundary.sequence)?;
        } else {
            persist_forwarding_index_best_effort(
                run_dir,
                &metadata.manifest.run_id,
                boundary.sequence,
                boundary.event_end_offset,
            );
        }
        Ok(true)
    })
}
