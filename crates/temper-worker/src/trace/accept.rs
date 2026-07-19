use std::collections::BTreeMap;

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityChildRecordV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentRunEventV1, BlobAttachmentV1, BlobReferenceV1,
    PromptCaptureDispositionV1,
};

use super::{
    MAX_CHILD_ACTIVITY_FRAME_BYTES, MAX_CHILD_ACTIVITY_RECORD_BYTES, RunState, TraceError,
    TraceRun, TraceRunInner, append_event, atomic_write, blob_path, canonicalize_child_scope,
    elapsed_ms, ensure_accepting, ensure_quota, event_blob_references, lock_spool, unlock_spool,
    validate_event_policy, validate_scope_acceptance,
};

impl TraceRun {
    pub fn accept_frame(&self, mut frame: AgentActivityFrameV1) -> Result<u64, TraceError> {
        frame.event.sanitize_retry_failure_message();
        frame.event.normalize_model_failure();
        let encoded_len = serde_json::to_vec(&frame)?.len();
        if encoded_len > MAX_CHILD_ACTIVITY_FRAME_BYTES {
            return Err(TraceError::InvalidSpool(format!(
                "child frame exceeds {MAX_CHILD_ACTIVITY_FRAME_BYTES} bytes"
            )));
        }
        frame.validate()?;
        self.accept_validated(frame, &[], false)
    }

    /// Accepts one complete attachment-bearing child record as an atomic trust
    /// boundary unit. Attachments are validated and preflighted with the event,
    /// durably stored idempotently, and only then may the event be appended.
    pub fn accept_record(&self, mut record: AgentActivityChildRecordV1) -> Result<u64, TraceError> {
        record.frame.event.sanitize_retry_failure_message();
        record.frame.event.normalize_model_failure();
        let encoded_len = serde_json::to_vec(&record)?.len();
        if encoded_len > MAX_CHILD_ACTIVITY_RECORD_BYTES {
            return Err(TraceError::InvalidSpool(format!(
                "child record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
            )));
        }
        record.validate()?;
        self.accept_validated(record.frame, &record.blobs, true)
    }

    fn accept_validated(
        &self,
        mut frame: AgentActivityFrameV1,
        attachments: &[BlobAttachmentV1],
        complete_record: bool,
    ) -> Result<u64, TraceError> {
        let mut state = self.inner.state.lock().expect("trace run state lock");
        ensure_accepting(&state)?;
        frame.scope = canonicalize_child_scope(
            &mut state.source_main_scope_id,
            &self.inner.manifest.main_scope,
            frame.scope,
        )?;
        validate_scope_acceptance(&state.scopes, &frame.scope)?;
        validate_event_policy(&self.inner.manifest.policy, &frame.event)?;

        let record_key = complete_record
            .then(|| serde_json::to_vec(&frame))
            .transpose()?;
        if let Some(seq) = record_key
            .as_ref()
            .and_then(|key| state.accepted_child_records.get(key))
        {
            return Ok(*seq);
        }
        if let AgentActivityEventV1::PromptPrepared(_) = &frame.event {
            if let Some((accepted, seq)) = state.accepted_prompts.get(&frame.scope.id) {
                return if accepted == &frame {
                    Ok(*seq)
                } else {
                    Err(TraceError::InvalidSpool(format!(
                        "scope {} already has a different prompt.prepared boundary",
                        frame.scope.id
                    )))
                };
            }
        }

        validate_record_blob_references(&state.blobs, attachments, &frame.event)?;
        let mut new_blobs = Vec::new();
        for attachment in attachments {
            match state.blobs.get(&attachment.blob.digest) {
                Some(existing) if existing == &attachment.blob => {}
                Some(_) => {
                    return Err(TraceError::InvalidSpool(
                        "one blob digest has conflicting metadata".to_string(),
                    ));
                }
                None => new_blobs.push((attachment, attachment.decode()?)),
            }
        }

        let seq = state.next_seq;
        let mut event = AgentRunEventV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.inner.manifest.run_id.clone(),
            seq,
            occurred_at: frame.occurred_at.clone(),
            elapsed_ms: elapsed_ms(self.inner.started),
            assignment: self.inner.manifest.assignment.clone(),
            agent_session_id: self.inner.manifest.agent_session_id.clone(),
            scope: frame.scope.clone(),
            turn: frame.turn,
            event: frame.event.clone(),
        };
        event.validate()?;
        let blob_bytes = new_blobs.iter().fold(0u64, |total, (attachment, _)| {
            total.saturating_add(attachment.blob.bytes)
        });
        let event_bytes = encoded_event_bytes(&event)?;
        if let Err(TraceError::QuotaExceeded) = ensure_quota(
            &self.inner.manifest.policy,
            state.used_bytes,
            blob_bytes.saturating_add(event_bytes),
            state.terminal_reserve,
        ) {
            let AgentActivityEventV1::PromptPrepared(prompt) = &mut event.event else {
                return Err(TraceError::QuotaExceeded);
            };
            if prompt.disposition != PromptCaptureDispositionV1::Captured {
                return Err(TraceError::QuotaExceeded);
            }
            prompt.disposition = PromptCaptureDispositionV1::OmittedQuota;
            prompt.captured_bytes = 0;
            prompt.content = None;
            event.validate()?;
            validate_event_policy(&self.inner.manifest.policy, &event.event)?;
            ensure_quota(
                &self.inner.manifest.policy,
                state.used_bytes,
                encoded_event_bytes(&event)?,
                state.terminal_reserve,
            )?;
            new_blobs.clear();
        }

        for (attachment, bytes) in new_blobs {
            store_blob_locked(&self.inner, &mut state, attachment, &bytes)?;
        }
        append_event(&self.inner, &mut state, &event, true)?;
        if let Some(key) = record_key {
            state.accepted_child_records.insert(key, seq);
        }
        if matches!(frame.event, AgentActivityEventV1::PromptPrepared(_)) {
            state
                .accepted_prompts
                .insert(frame.scope.id.clone(), (frame, seq));
        }
        Ok(seq)
    }
}

fn encoded_event_bytes(event: &AgentRunEventV1) -> Result<u64, TraceError> {
    Ok(u64::try_from(serde_json::to_vec(event)?.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1))
}

fn store_blob_locked(
    inner: &TraceRunInner,
    state: &mut RunState,
    attachment: &BlobAttachmentV1,
    bytes: &[u8],
) -> Result<(), TraceError> {
    let path = blob_path(&inner.blobs_dir, &attachment.blob)?;
    lock_spool(inner)?;
    if let Err(error) = atomic_write(&path, bytes, false) {
        let _ = unlock_spool(inner);
        state.disabled = true;
        return Err(error);
    }
    unlock_spool(inner)?;
    state.used_bytes = state.used_bytes.saturating_add(attachment.blob.bytes);
    state
        .blobs
        .insert(attachment.blob.digest.clone(), attachment.blob.clone());
    Ok(())
}

fn validate_record_blob_references(
    blobs: &BTreeMap<String, BlobReferenceV1>,
    attachments: &[BlobAttachmentV1],
    event: &AgentActivityEventV1,
) -> Result<(), TraceError> {
    for reference in event_blob_references(event) {
        let stored = blobs.get(&reference.digest) == Some(reference);
        let attached = attachments
            .iter()
            .any(|attachment| &attachment.blob == reference);
        if !stored && !attached {
            return Err(TraceError::InvalidSpool(format!(
                "event references unavailable blob {}",
                reference.digest
            )));
        }
    }
    Ok(())
}
