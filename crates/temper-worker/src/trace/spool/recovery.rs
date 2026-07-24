use super::*;

impl RecoveredForwardingRun {
    fn into_public(self) -> RecoveredTraceRun {
        RecoveredTraceRun {
            manifest: self.manifest,
            events: self.events,
            blobs: self.blobs,
            acknowledged_seq: self.acknowledged_seq,
        }
    }
}

pub(in crate::trace) fn recover_run(run_dir: &Path) -> Result<RecoveredTraceRun, TraceError> {
    repair_run_permissions(run_dir)?;
    let (lock_path, lock_file) = open_spool_lock(run_dir)?;
    lock_file
        .lock_exclusive()
        .map_err(|source| io_error("lock trace spool for recovery", &lock_path, source))?;
    let recovered = recover_run_with_offsets_locked(run_dir).map(|run| run.into_public());
    let unlocked = fs2::FileExt::unlock(&lock_file)
        .map_err(|source| io_error("unlock trace spool after recovery", &lock_path, source));
    match (recovered, unlocked) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(run), Ok(())) => Ok(run),
    }
}

/// Recovers one run for forwarding while holding its spool lock. A matching
/// index at EOF returns metadata only; a grown file validates just its suffix.
pub(in crate::trace) fn recover_forwarding_run(
    run_dir: &Path,
    repair_permissions: bool,
) -> Result<RecoveredForwardingRun, TraceError> {
    if repair_permissions {
        repair_run_permissions(run_dir)?;
    }
    let (lock_path, lock_file) = open_spool_lock(run_dir)?;
    lock_file.lock_exclusive().map_err(|source| {
        io_error(
            "lock trace spool for forwarding recovery",
            &lock_path,
            source,
        )
    })?;
    let recovered = recover_forwarding_run_locked(run_dir);
    let unlocked = fs2::FileExt::unlock(&lock_file).map_err(|source| {
        io_error(
            "unlock trace spool after forwarding recovery",
            &lock_path,
            source,
        )
    });
    match (recovered, unlocked) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(run), Ok(())) => Ok(run),
    }
}

fn recover_forwarding_run_locked(run_dir: &Path) -> Result<RecoveredForwardingRun, TraceError> {
    let metadata = recover_spool_metadata(run_dir)?;
    if recover_compacted_marker(run_dir, &metadata)? {
        return Ok(empty_forwarding_run(metadata));
    }

    let events_path = run_dir.join("events.jsonl");
    let event_file_len = fs::metadata(&events_path)
        .map_err(|source| io_error("inspect activity records", &events_path, source))?
        .len();
    if let Some(index) = read_valid_forwarding_index(run_dir, &metadata, event_file_len) {
        if index.event_end_offset == event_file_len {
            return Ok(empty_forwarding_run(metadata));
        }
        let (events, event_end_offsets) =
            recover_event_records(&events_path, index.event_end_offset)?;
        validate_forwarding_suffix(&metadata, &events)?;
        let blobs = recover_referenced_blobs(run_dir, &events)?;
        return Ok(RecoveredForwardingRun {
            manifest: metadata.manifest,
            events,
            event_end_offsets,
            blobs,
            acknowledged_seq: metadata.cursor.highest_contiguous_seq,
        });
    }

    // Missing, malformed, stale, or impossible derived metadata never makes a
    // spool corrupt. Fall back to authoritative full recovery and converge by
    // replacing the sidecar only after all payload validation succeeds.
    let recovered = recover_run_payload_locked(run_dir, metadata)?;
    if recovered.events.is_empty() {
        return Ok(recovered);
    }
    let last = recovered.events.last().expect("non-empty recovered run");
    if recovered.acknowledged_seq == last.seq && last.event.is_terminal() {
        compact_fully_acknowledged_run(
            run_dir,
            &recovered.manifest.run_id,
            recovered.acknowledged_seq,
        )?;
        return Ok(RecoveredForwardingRun {
            manifest: recovered.manifest,
            events: Vec::new(),
            event_end_offsets: Vec::new(),
            blobs: Vec::new(),
            acknowledged_seq: recovered.acknowledged_seq,
        });
    }
    let boundary = acknowledged_event_end_offset(&recovered)?;
    persist_forwarding_index_best_effort(
        run_dir,
        &recovered.manifest.run_id,
        recovered.acknowledged_seq,
        boundary,
    );
    Ok(recovered)
}

pub(super) fn recover_run_with_offsets_locked(
    run_dir: &Path,
) -> Result<RecoveredForwardingRun, TraceError> {
    let metadata = recover_spool_metadata(run_dir)?;
    if recover_compacted_marker(run_dir, &metadata)? {
        return Ok(empty_forwarding_run(metadata));
    }
    recover_run_payload_locked(run_dir, metadata)
}

fn recover_run_payload_locked(
    run_dir: &Path,
    metadata: RecoveredSpoolMetadata,
) -> Result<RecoveredForwardingRun, TraceError> {
    let events_path = run_dir.join("events.jsonl");
    let (events, event_end_offsets) = recover_event_records(&events_path, 0)?;
    if events.is_empty() || !matches!(events[0].event, AgentActivityEventV1::RunStarted(_)) {
        return Err(TraceError::InvalidSpool(format!(
            "run {} does not begin with run.started",
            metadata.manifest.run_id
        )));
    }
    temper_protocol_activity::validate_run_stream(&events)?;
    validate_event_identities_and_terminal(&metadata.manifest, &events)?;

    let last_seq = events.last().map_or(0, |event| event.seq);
    if metadata.cursor.highest_contiguous_seq > last_seq {
        return Err(TraceError::InvalidAcknowledgement {
            acknowledged: metadata.cursor.highest_contiguous_seq,
            last_seq,
        });
    }
    let blobs = recover_referenced_blobs(run_dir, &events)?;
    Ok(RecoveredForwardingRun {
        manifest: metadata.manifest,
        events,
        event_end_offsets,
        blobs,
        acknowledged_seq: metadata.cursor.highest_contiguous_seq,
    })
}

pub(super) fn recover_spool_metadata(run_dir: &Path) -> Result<RecoveredSpoolMetadata, TraceError> {
    let manifest_path = run_dir.join("manifest.json");
    let manifest: TraceManifestV1 = read_json(&manifest_path)?;
    validate_manifest(&manifest)?;
    if run_dir.file_name().and_then(|name| name.to_str()) != Some(manifest.run_id.as_str()) {
        return Err(TraceError::InvalidSpool(format!(
            "run directory does not match manifest run ID {}",
            manifest.run_id
        )));
    }

    let cursor_path = run_dir.join("acknowledgement.json");
    let cursor: TraceAckCursorV1 = read_json(&cursor_path)?;
    if cursor.version != ACTIVITY_PROTOCOL_VERSION || cursor.run_id != manifest.run_id {
        return Err(TraceError::InvalidSpool(format!(
            "run {} has a mismatched acknowledgement cursor",
            manifest.run_id
        )));
    }
    Ok(RecoveredSpoolMetadata { manifest, cursor })
}

pub(super) fn recover_compacted_marker(
    run_dir: &Path,
    metadata: &RecoveredSpoolMetadata,
) -> Result<bool, TraceError> {
    let compacted_path = run_dir.join("compacted.json");
    if !compacted_path.exists() {
        return Ok(false);
    }
    let compacted: TraceCompactedAckV1 = read_json(&compacted_path)?;
    if compacted.version != ACTIVITY_PROTOCOL_VERSION
        || compacted.run_id != metadata.manifest.run_id
        || compacted.highest_contiguous_seq == 0
        || compacted.highest_contiguous_seq != metadata.cursor.highest_contiguous_seq
        || !compacted.terminal
    {
        return Err(TraceError::InvalidSpool(format!(
            "run {} has a mismatched compact acknowledgement marker",
            metadata.manifest.run_id
        )));
    }
    reclaim_compacted_payload(run_dir)?;
    Ok(true)
}

fn empty_forwarding_run(metadata: RecoveredSpoolMetadata) -> RecoveredForwardingRun {
    RecoveredForwardingRun {
        manifest: metadata.manifest,
        events: Vec::new(),
        event_end_offsets: Vec::new(),
        blobs: Vec::new(),
        acknowledged_seq: metadata.cursor.highest_contiguous_seq,
    }
}

fn validate_event_identities_and_terminal(
    manifest: &TraceManifestV1,
    events: &[AgentRunEventV1],
) -> Result<(), TraceError> {
    let mut terminal_seen = false;
    for event in events {
        if event.run_id != manifest.run_id
            || event.assignment != manifest.assignment
            || event.agent_session_id != manifest.agent_session_id
        {
            return Err(TraceError::InvalidSpool(format!(
                "run {} event identity differs from immutable manifest",
                manifest.run_id
            )));
        }
        if terminal_seen {
            return Err(TraceError::InvalidSpool(format!(
                "run {} contains an event after its terminal event",
                manifest.run_id
            )));
        }
        terminal_seen = event.event.is_terminal();
    }
    Ok(())
}

fn validate_forwarding_suffix(
    metadata: &RecoveredSpoolMetadata,
    events: &[AgentRunEventV1],
) -> Result<(), TraceError> {
    if events.is_empty() {
        return Ok(());
    }
    if metadata.cursor.highest_contiguous_seq == 0 {
        if !matches!(events[0].event, AgentActivityEventV1::RunStarted(_)) {
            return Err(TraceError::InvalidSpool(format!(
                "run {} does not begin with run.started",
                metadata.manifest.run_id
            )));
        }
        temper_protocol_activity::validate_run_stream(events)?;
    } else {
        let mut expected = metadata.cursor.highest_contiguous_seq.saturating_add(1);
        for event in events {
            event.validate()?;
            if event.seq != expected {
                return Err(TraceError::InvalidSpool(format!(
                    "run {} forwarding suffix is not contiguous at sequence {}",
                    metadata.manifest.run_id, event.seq
                )));
            }
            expected = expected.saturating_add(1);
        }
    }
    validate_event_identities_and_terminal(&metadata.manifest, events)
}

pub(super) fn recover_referenced_blobs(
    run_dir: &Path,
    events: &[AgentRunEventV1],
) -> Result<Vec<BlobAttachmentV1>, TraceError> {
    let mut references = BTreeMap::<String, BlobReferenceV1>::new();
    for event in events {
        for reference in event_blob_references(&event.event) {
            if references
                .insert(reference.digest.clone(), reference.clone())
                .is_some_and(|existing| existing != *reference)
            {
                return Err(TraceError::InvalidSpool(
                    "one recovered blob digest has conflicting metadata".to_string(),
                ));
            }
        }
    }
    let blobs_dir = run_dir.join("blobs");
    let mut blobs = Vec::with_capacity(references.len());
    for reference in references.values() {
        let path = blob_path(&blobs_dir, reference)?;
        let bytes = read_blob_bytes(&path)?;
        let attachment = BlobAttachmentV1::from_bytes(reference.media_type, &bytes);
        if attachment.blob != *reference {
            return Err(TraceError::InvalidSpool(format!(
                "recovered blob {} does not match its event reference",
                reference.digest
            )));
        }
        blobs.push(attachment);
    }
    Ok(blobs)
}

fn read_valid_forwarding_index(
    run_dir: &Path,
    metadata: &RecoveredSpoolMetadata,
    event_file_len: u64,
) -> Option<TraceForwardingIndexV1> {
    let path = run_dir.join(FORWARDING_INDEX_FILE);
    let file_type = fs::symlink_metadata(&path).ok()?.file_type();
    if !file_type.is_file() || file_type.is_symlink() {
        return None;
    }
    let index = serde_json::from_slice::<TraceForwardingIndexV1>(&fs::read(path).ok()?).ok()?;
    if index.version != FORWARDING_INDEX_VERSION
        || index.run_id != metadata.manifest.run_id
        || index.highest_contiguous_seq != metadata.cursor.highest_contiguous_seq
        || index.event_end_offset > event_file_len
        || (index.highest_contiguous_seq == 0 && index.event_end_offset != 0)
        || (index.highest_contiguous_seq > 0 && index.event_end_offset == 0)
    {
        return None;
    }
    Some(index)
}

fn acknowledged_event_end_offset(recovered: &RecoveredForwardingRun) -> Result<u64, TraceError> {
    if recovered.acknowledged_seq == 0 {
        return Ok(0);
    }
    recovered
        .events
        .iter()
        .zip(&recovered.event_end_offsets)
        .find_map(|(event, offset)| (event.seq == recovered.acknowledged_seq).then_some(*offset))
        .ok_or_else(|| {
            TraceError::InvalidSpool(format!(
                "run {} has no event boundary for acknowledgement {}",
                recovered.manifest.run_id, recovered.acknowledged_seq
            ))
        })
}
