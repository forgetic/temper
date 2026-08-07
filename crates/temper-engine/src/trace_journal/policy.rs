fn validate_binding(binding: &AuthenticatedWorkerBinding) -> Result<(), TraceJournalError> {
    validate_short_identifier(&binding.worker_id, "worker_id")?;
    validate_short_identifier(&binding.assignment_id, "assignment_id")?;
    binding.capture_policy.validate()?;
    Ok(())
}

fn validate_short_identifier(value: &str, field: &str) -> Result<(), TraceJournalError> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(TraceJournalError::InvalidBinding(format!(
            "{field} must be a non-empty, bounded identifier without control characters"
        )));
    }
    Ok(())
}

fn validate_manifest(manifest: &AgentTraceManifest) -> Result<(), TraceJournalError> {
    if manifest.format_version != JOURNAL_FORMAT_VERSION {
        return Err(TraceJournalError::CorruptRun(format!(
            "unsupported manifest version {}",
            manifest.format_version
        )));
    }
    validate_short_identifier(&manifest.worker_id, "manifest.worker_id")?;
    validate_short_identifier(&manifest.assignment_id, "manifest.assignment_id")?;
    manifest.capture_policy.validate()?;
    DateTime::parse_from_rfc3339(&manifest.created_at).map_err(|error| {
        TraceJournalError::CorruptRun(format!("manifest creation time is invalid: {error}"))
    })?;
    Ok(())
}

fn manifest_matches_binding(
    manifest: &AgentTraceManifest,
    binding: &AuthenticatedWorkerBinding,
) -> bool {
    manifest.worker_id == binding.worker_id
        && manifest.assignment_id == binding.assignment_id
        && manifest.assignment == binding.assignment
        && manifest.agent_session_id == binding.agent_session_id
        && manifest.capture_policy == binding.capture_policy
}

fn validate_stream_for_manifest(
    events: &[AgentRunEventV1],
    manifest: &AgentTraceManifest,
) -> Result<(), TraceJournalError> {
    if events.is_empty() {
        return Ok(());
    }
    validate_run_stream(events)?;
    validate_content_reference_metadata(events)?;
    let mut terminal_seq = None;
    for event in events {
        if event.run_id != manifest.run_id
            || event.assignment != manifest.assignment
            || event.agent_session_id != manifest.agent_session_id
        {
            return Err(TraceJournalError::BindingMismatch);
        }
        validate_event_policy(event, &manifest.capture_policy)?;
        if event.seq == 1 {
            let AgentActivityEventV1::RunStarted(started) = &event.event else {
                return Err(TraceJournalError::TerminalConsistency(
                    "sequence 1 must be run.started".to_string(),
                ));
            };
            if started.capture != manifest.capture_policy.capture {
                return Err(TraceJournalError::PolicyViolation(
                    "run.started capture mode differs from the manifest".to_string(),
                ));
            }
        } else if matches!(event.event, AgentActivityEventV1::RunStarted(_)) {
            return Err(TraceJournalError::TerminalConsistency(
                "run.started may appear only at sequence 1".to_string(),
            ));
        }
        if event.event.is_terminal() {
            if terminal_seq.replace(event.seq).is_some() {
                return Err(TraceJournalError::TerminalConsistency(
                    "a run may contain exactly one terminal event".to_string(),
                ));
            }
        } else if terminal_seq.is_some() {
            return Err(TraceJournalError::TerminalConsistency(
                "events may not follow a terminal event".to_string(),
            ));
        }
    }
    if terminal_seq.is_some_and(|seq| seq != events.last().map_or(0, |event| event.seq)) {
        return Err(TraceJournalError::TerminalConsistency(
            "terminal event must be the final sequence".to_string(),
        ));
    }
    Ok(())
}

fn validate_event_policy(
    event: &AgentRunEventV1,
    policy: &AgentActivityCapturePolicyV1,
) -> Result<(), TraceJournalError> {
    match policy.capture {
        CaptureModeV1::Off => {
            return Err(TraceJournalError::PolicyViolation(
                "events cannot be stored while capture is off".to_string(),
            ));
        }
        CaptureModeV1::Metadata => match &event.event {
            AgentActivityEventV1::PromptPrepared(value)
                if value.disposition != PromptCaptureDispositionV1::OmittedPolicy =>
            {
                return Err(TraceJournalError::PolicyViolation(
                    "metadata capture contains a non-policy prompt disposition".to_string(),
                ));
            }
            AgentActivityEventV1::AssistantMessage(_)
            | AgentActivityEventV1::OutputTextDelta(_)
            | AgentActivityEventV1::OutputThinkingDelta(_) => {
                return Err(TraceJournalError::PolicyViolation(
                    "metadata capture contains transcript content".to_string(),
                ));
            }
            AgentActivityEventV1::ToolStarted(value) if value.arguments.is_some() => {
                return Err(TraceJournalError::PolicyViolation(
                    "metadata capture contains tool arguments".to_string(),
                ));
            }
            AgentActivityEventV1::ToolFinished(value) if value.result.is_some() => {
                return Err(TraceJournalError::PolicyViolation(
                    "metadata capture contains a tool result".to_string(),
                ));
            }
            AgentActivityEventV1::SteeringApplied(value) if value.instruction.is_some() => {
                return Err(TraceJournalError::PolicyViolation(
                    "metadata capture contains a steering instruction".to_string(),
                ));
            }
            _ => {}
        },
        CaptureModeV1::Transcript => {
            if matches!(
                event.event,
                AgentActivityEventV1::OutputTextDelta(_)
                    | AgentActivityEventV1::OutputThinkingDelta(_)
            ) {
                return Err(TraceJournalError::PolicyViolation(
                    "transcript capture contains diagnostic deltas".to_string(),
                ));
            }
            if matches!(
                &event.event,
                AgentActivityEventV1::PromptPrepared(value)
                    if value.disposition == PromptCaptureDispositionV1::OmittedPolicy
            ) {
                return Err(TraceJournalError::PolicyViolation(
                    "transcript capture contains a policy-omitted prompt".to_string(),
                ));
            }
        }
        CaptureModeV1::Diagnostic => {
            if matches!(
                &event.event,
                AgentActivityEventV1::PromptPrepared(value)
                    if value.disposition == PromptCaptureDispositionV1::OmittedPolicy
            ) {
                return Err(TraceJournalError::PolicyViolation(
                    "diagnostic capture contains a policy-omitted prompt".to_string(),
                ));
            }
            if !policy.capture_thinking
                && matches!(event.event, AgentActivityEventV1::OutputThinkingDelta(_))
            {
                return Err(TraceJournalError::PolicyViolation(
                    "thinking capture is disabled".to_string(),
                ));
            }
        }
    }
    for content in captured_contents(event) {
        match content {
            CapturedContentV1::Inline(inline)
                if inline.text.len() > policy.max_inline_bytes as usize =>
            {
                return Err(TraceJournalError::PolicyViolation(
                    "inline content exceeds the bound manifest policy".to_string(),
                ));
            }
            CapturedContentV1::Blob { blob } if blob.bytes > policy.max_blob_bytes => {
                return Err(TraceJournalError::PolicyViolation(
                    "blob content exceeds the bound manifest policy".to_string(),
                ));
            }
            _ => {}
        }
    }
    match &event.event {
        AgentActivityEventV1::OutputTextDelta(value)
        | AgentActivityEventV1::OutputThinkingDelta(value)
            if value.delta.text.len() > policy.max_inline_bytes as usize =>
        {
            return Err(TraceJournalError::PolicyViolation(
                "delta content exceeds the bound manifest policy".to_string(),
            ));
        }
        AgentActivityEventV1::ModelCallRetrying(value)
            if value.failure.message != MODEL_CALL_RETRY_FAILURE_MESSAGE =>
        {
            return Err(TraceJournalError::PolicyViolation(
                "retry failure message is not the allowlisted summary".to_string(),
            ));
        }
        AgentActivityEventV1::RunFailed(value)
            if value.failure.message.len() > policy.max_inline_bytes as usize =>
        {
            return Err(TraceJournalError::PolicyViolation(
                "terminal failure detail exceeds the bound manifest policy".to_string(),
            ));
        }
        _ => {}
    }
    Ok(())
}

fn sanitize_for_policy(
    mut event: AgentRunEventV1,
    policy: &AgentActivityCapturePolicyV1,
) -> AgentRunEventV1 {
    // Re-establish the privacy invariant at the engine boundary. This runs for
    // every capture mode and for direct/forged batches that bypass the worker.
    event.event.sanitize_untrusted_activity();
    match policy.capture {
        CaptureModeV1::Off => {}
        CaptureModeV1::Metadata => match &mut event.event {
            AgentActivityEventV1::PromptPrepared(value)
                if value.disposition == PromptCaptureDispositionV1::Captured =>
            {
                omit_prompt(value, PromptCaptureDispositionV1::OmittedPolicy);
            }
            AgentActivityEventV1::AssistantMessage(value) => {
                event.event =
                    omission_gap(DroppedEventKindV1::TextDelta, content_bytes(&value.content));
            }
            AgentActivityEventV1::OutputTextDelta(value) => {
                event.event =
                    omission_gap(DroppedEventKindV1::TextDelta, value.delta.text.len() as u64);
            }
            AgentActivityEventV1::OutputThinkingDelta(value) => {
                event.event = omission_gap(
                    DroppedEventKindV1::ThinkingDelta,
                    value.delta.text.len() as u64,
                );
            }
            AgentActivityEventV1::ToolStarted(value) => value.arguments = None,
            AgentActivityEventV1::ToolFinished(value) => value.result = None,
            AgentActivityEventV1::SteeringApplied(value) => value.instruction = None,
            _ => {}
        },
        CaptureModeV1::Transcript => match &mut event.event {
            AgentActivityEventV1::OutputTextDelta(value) => {
                event.event =
                    omission_gap(DroppedEventKindV1::TextDelta, value.delta.text.len() as u64);
            }
            AgentActivityEventV1::OutputThinkingDelta(value) => {
                event.event = omission_gap(
                    DroppedEventKindV1::ThinkingDelta,
                    value.delta.text.len() as u64,
                );
            }
            _ => {}
        },
        CaptureModeV1::Diagnostic => {
            if !policy.capture_thinking {
                if let AgentActivityEventV1::OutputThinkingDelta(value) = &event.event {
                    event.event = omission_gap(
                        DroppedEventKindV1::ThinkingDelta,
                        value.delta.text.len() as u64,
                    );
                }
            }
        }
    }

    match &mut event.event {
        AgentActivityEventV1::PromptPrepared(value)
            if value
                .content
                .as_ref()
                .is_some_and(|content| content_exceeds_policy(content, policy)) =>
        {
            omit_prompt(value, PromptCaptureDispositionV1::OmittedLimit);
        }
        AgentActivityEventV1::AssistantMessage(value)
            if content_exceeds_policy(&value.content, policy) =>
        {
            event.event =
                omission_gap(DroppedEventKindV1::TextDelta, content_bytes(&value.content));
        }
        AgentActivityEventV1::ToolStarted(value)
            if value
                .arguments
                .as_ref()
                .is_some_and(|content| content_exceeds_policy(content, policy)) =>
        {
            value.arguments = None;
        }
        AgentActivityEventV1::ToolFinished(value)
            if value
                .result
                .as_ref()
                .is_some_and(|content| content_exceeds_policy(content, policy)) =>
        {
            value.result = None;
        }
        AgentActivityEventV1::SteeringApplied(value)
            if value
                .instruction
                .as_ref()
                .is_some_and(|content| content_exceeds_policy(content, policy)) =>
        {
            value.instruction = None;
        }
        AgentActivityEventV1::OutputTextDelta(value)
            if value.delta.text.len() > policy.max_inline_bytes as usize =>
        {
            event.event =
                omission_gap(DroppedEventKindV1::TextDelta, value.delta.text.len() as u64);
        }
        AgentActivityEventV1::OutputThinkingDelta(value)
            if value.delta.text.len() > policy.max_inline_bytes as usize =>
        {
            event.event = omission_gap(
                DroppedEventKindV1::ThinkingDelta,
                value.delta.text.len() as u64,
            );
        }
        AgentActivityEventV1::ModelCallRetrying(value) => {
            value.failure.message = MODEL_CALL_RETRY_FAILURE_MESSAGE.to_string();
        }
        AgentActivityEventV1::RunFailed(value)
            if value.failure.message.len() > policy.max_inline_bytes as usize =>
        {
            value.failure.message = omission_message(policy.max_inline_bytes);
        }
        _ => {}
    }
    event
}

fn strip_optional_content(mut event: AgentRunEventV1) -> AgentRunEventV1 {
    match &mut event.event {
        AgentActivityEventV1::PromptPrepared(value)
            if value.disposition == PromptCaptureDispositionV1::Captured =>
        {
            omit_prompt(value, PromptCaptureDispositionV1::OmittedQuota);
        }
        AgentActivityEventV1::AssistantMessage(value) => {
            event.event =
                omission_gap(DroppedEventKindV1::TextDelta, content_bytes(&value.content));
        }
        AgentActivityEventV1::OutputTextDelta(value) => {
            event.event =
                omission_gap(DroppedEventKindV1::TextDelta, value.delta.text.len() as u64);
        }
        AgentActivityEventV1::OutputThinkingDelta(value) => {
            event.event = omission_gap(
                DroppedEventKindV1::ThinkingDelta,
                value.delta.text.len() as u64,
            );
        }
        AgentActivityEventV1::ToolStarted(value) => value.arguments = None,
        AgentActivityEventV1::ToolFinished(value) => value.result = None,
        AgentActivityEventV1::SteeringApplied(value) => value.instruction = None,
        AgentActivityEventV1::ModelCallRetrying(value) => {
            value.failure.message = MODEL_CALL_RETRY_FAILURE_MESSAGE.to_string();
        }
        AgentActivityEventV1::RunFailed(value) => {
            value.failure.message = omission_message(1);
        }
        _ => {}
    }
    event
}

fn omit_prompt(
    value: &mut temper_protocol_activity::PromptPreparedV1,
    disposition: PromptCaptureDispositionV1,
) {
    value.disposition = disposition;
    value.captured_bytes = 0;
    value.content = None;
}

fn omission_message(max_inline_bytes: u32) -> String {
    const MARKER: &str = "[omitted]";
    if max_inline_bytes as usize >= MARKER.len() {
        MARKER.to_string()
    } else {
        ".".to_string()
    }
}

fn omission_gap(kind: DroppedEventKindV1, bytes: u64) -> AgentActivityEventV1 {
    AgentActivityEventV1::TraceGap(TraceGapV1 {
        dropped_events: 1,
        dropped_bytes: bytes.max(OMISSION_MARKER_BYTES),
        kinds: vec![kind],
    })
}

fn content_exceeds_policy(
    content: &CapturedContentV1,
    policy: &AgentActivityCapturePolicyV1,
) -> bool {
    match content {
        CapturedContentV1::Inline(value) => value.text.len() > policy.max_inline_bytes as usize,
        CapturedContentV1::Blob { blob } => blob.bytes > policy.max_blob_bytes,
    }
}

fn content_bytes(content: &CapturedContentV1) -> u64 {
    match content {
        CapturedContentV1::Inline(value) => value.text.len() as u64,
        CapturedContentV1::Blob { blob } => blob.bytes,
    }
}

fn captured_contents(event: &AgentRunEventV1) -> Vec<&CapturedContentV1> {
    match &event.event {
        AgentActivityEventV1::PromptPrepared(value) => value.content.iter().collect(),
        AgentActivityEventV1::AssistantMessage(value) => vec![&value.content],
        AgentActivityEventV1::ToolStarted(value) => value.arguments.iter().collect(),
        AgentActivityEventV1::ToolFinished(value) => value.result.iter().collect(),
        AgentActivityEventV1::SteeringApplied(value) => value.instruction.iter().collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn content_references(event: &AgentRunEventV1) -> Vec<&BlobReferenceV1> {
    captured_contents(event)
        .into_iter()
        .filter_map(|content| match content {
            CapturedContentV1::Blob { blob } => Some(blob),
            CapturedContentV1::Inline(_) => None,
        })
        .collect()
}

fn validate_content_reference_metadata(
    events: &[AgentRunEventV1],
) -> Result<(), TraceJournalError> {
    let mut references = BTreeMap::<&str, &BlobReferenceV1>::new();
    for event in events {
        for reference in content_references(event) {
            if references
                .insert(reference.digest.as_str(), reference)
                .is_some_and(|existing| existing != reference)
            {
                return Err(TraceJournalError::CorruptRun(format!(
                    "blob digest {} has conflicting metadata in the event stream",
                    reference.digest
                )));
            }
        }
    }
    Ok(())
}
