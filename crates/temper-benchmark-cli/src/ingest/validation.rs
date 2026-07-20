// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_protocol_activity::{
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeV1,
    BlobAttachmentV1, BlobReferenceV1, validate_run_stream, validate_scope_ancestry,
};

use super::diagnostics::{
    content_references, record_call_diagnostics, record_evidence_diagnostics,
    record_truncated_content, warning,
};
use super::{NormalizedTrace, TraceIngestError};
use crate::{TraceDiagnosticCodeV1, TraceDiagnosticV1, TraceInputKindV1};

pub(super) fn finish_normalization(
    source: TraceInputKindV1,
    events: Vec<AgentRunEventV1>,
    supplied_attachments: Vec<BlobAttachmentV1>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<NormalizedTrace, TraceIngestError> {
    finish_normalization_with_evidence(source, events, supplied_attachments, diagnostics, true)
}

pub(super) fn finish_worker_normalization(
    source: TraceInputKindV1,
    events: Vec<AgentRunEventV1>,
    supplied_attachments: Vec<BlobAttachmentV1>,
) -> Result<NormalizedTrace, TraceIngestError> {
    let mut diagnostics = Vec::new();
    finish_normalization_with_evidence(
        source,
        events,
        supplied_attachments,
        &mut diagnostics,
        false,
    )
}

fn finish_normalization_with_evidence(
    source: TraceInputKindV1,
    events: Vec<AgentRunEventV1>,
    supplied_attachments: Vec<BlobAttachmentV1>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
    record_unavailable_enrichments: bool,
) -> Result<NormalizedTrace, TraceIngestError> {
    validate_events(&events, diagnostics)?;
    let references = reference_map(&events)?;
    let attachments = validate_attachments(&references, supplied_attachments)?;
    record_call_diagnostics(&events, diagnostics)?;
    if record_unavailable_enrichments {
        record_evidence_diagnostics(diagnostics);
    }

    Ok(NormalizedTrace {
        source,
        events,
        attachments,
        diagnostics: std::mem::take(diagnostics),
    })
}

struct StreamState<'a> {
    run_id: &'a str,
    assignment: &'a AgentAssignmentIdentityV1,
    session_id: &'a Option<String>,
    previous_seq: Option<u64>,
    previous_elapsed: Option<u64>,
    run_started: bool,
    terminal_seq: Option<u64>,
    has_gap: bool,
}

fn validate_events(
    events: &[AgentRunEventV1],
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<(), TraceIngestError> {
    let first = events.first().ok_or_else(|| {
        TraceIngestError::InvalidStream("a normalized stream must contain an event".to_string())
    })?;
    let mut state = StreamState {
        run_id: &first.run_id,
        assignment: &first.assignment,
        session_id: &first.agent_session_id,
        previous_seq: None,
        previous_elapsed: None,
        run_started: false,
        terminal_seq: None,
        has_gap: false,
    };
    for event in events {
        validate_event(event, &mut state, diagnostics)?;
    }

    let scopes = events
        .iter()
        .map(|event| event.scope.clone())
        .collect::<Vec<AgentScopeV1>>();
    validate_scope_ancestry(&scopes)
        .map_err(|error| TraceIngestError::InvalidStream(error.to_string()))?;
    if !state.has_gap {
        validate_run_stream(events)
            .map_err(|error| TraceIngestError::InvalidStream(error.to_string()))?;
    }
    record_missing_boundaries(&state, diagnostics);
    Ok(())
}

fn validate_event(
    event: &AgentRunEventV1,
    state: &mut StreamState<'_>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<(), TraceIngestError> {
    event
        .validate()
        .map_err(|error| TraceIngestError::InvalidActivity {
            seq: event.seq,
            detail: error.to_string(),
        })?;
    validate_identity(event, state)?;
    validate_order(event, state, diagnostics)?;
    validate_lifecycle(event, state, diagnostics)?;
    record_truncated_content(event, diagnostics);
    state.previous_seq = Some(event.seq);
    state.previous_elapsed = Some(event.elapsed_ms);
    Ok(())
}

fn validate_identity(
    event: &AgentRunEventV1,
    state: &StreamState<'_>,
) -> Result<(), TraceIngestError> {
    let changed = if event.run_id != state.run_id {
        Some("run identity")
    } else if &event.assignment != state.assignment {
        Some("assignment identity")
    } else if &event.agent_session_id != state.session_id {
        Some("agent session identity")
    } else {
        None
    };
    if let Some(identity) = changed {
        Err(TraceIngestError::InvalidStream(format!(
            "{identity} changed at sequence {}",
            event.seq
        )))
    } else {
        Ok(())
    }
}

fn validate_order(
    event: &AgentRunEventV1,
    state: &mut StreamState<'_>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<(), TraceIngestError> {
    if let Some(previous) = state.previous_seq {
        if event.seq <= previous {
            return Err(TraceIngestError::InvalidStream(format!(
                "sequence {} does not increase after {previous}",
                event.seq
            )));
        }
        if event.seq != previous + 1 {
            state.has_gap = true;
            diagnostics.push(warning(
                TraceDiagnosticCodeV1::SequenceGap,
                format!(
                    "event sequence jumps from {previous} to {}; missing {} record(s)",
                    event.seq,
                    event.seq - previous - 1
                ),
                Some(event.seq),
            ));
        }
    } else if event.seq != 1 {
        state.has_gap = true;
        diagnostics.push(warning(
            TraceDiagnosticCodeV1::SequenceGap,
            format!("event stream starts at sequence {}; expected 1", event.seq),
            Some(event.seq),
        ));
    }
    if state
        .previous_elapsed
        .is_some_and(|elapsed| event.elapsed_ms < elapsed)
    {
        return Err(TraceIngestError::InvalidStream(format!(
            "elapsed time decreases at sequence {}",
            event.seq
        )));
    }
    Ok(())
}

fn validate_lifecycle(
    event: &AgentRunEventV1,
    state: &mut StreamState<'_>,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) -> Result<(), TraceIngestError> {
    if let Some(terminal) = state.terminal_seq {
        return Err(TraceIngestError::InvalidStream(format!(
            "event sequence {} appears after terminal sequence {terminal}",
            event.seq
        )));
    }
    match &event.event {
        AgentActivityEventV1::RunStarted(_) => {
            if state.run_started {
                return Err(TraceIngestError::InvalidStream(format!(
                    "run.started is repeated at sequence {}",
                    event.seq
                )));
            }
            state.run_started = true;
        }
        AgentActivityEventV1::RunFinished(_) | AgentActivityEventV1::RunFailed(_) => {
            state.terminal_seq = Some(event.seq);
        }
        AgentActivityEventV1::TraceGap(gap) => diagnostics.push(warning(
            TraceDiagnosticCodeV1::TraceGap,
            format!(
                "producer reported {} dropped event(s) and {} dropped byte(s)",
                gap.dropped_events, gap.dropped_bytes
            ),
            Some(event.seq),
        )),
        _ => {}
    }
    Ok(())
}

fn record_missing_boundaries(state: &StreamState<'_>, diagnostics: &mut Vec<TraceDiagnosticV1>) {
    if !state.run_started {
        diagnostics.push(warning(
            TraceDiagnosticCodeV1::MissingRunStart,
            "trace does not contain run.started".to_string(),
            None,
        ));
    }
    if state.terminal_seq.is_none() {
        diagnostics.push(warning(
            TraceDiagnosticCodeV1::MissingTerminalEvent,
            "trace ended without run.finished or run.failed".to_string(),
            None,
        ));
    }
}

pub(super) fn reference_map(
    events: &[AgentRunEventV1],
) -> Result<BTreeMap<String, BlobReferenceV1>, TraceIngestError> {
    let mut references = BTreeMap::new();
    for event in events {
        for reference in content_references(event) {
            reference
                .validate()
                .map_err(|error| TraceIngestError::Attachment(error.to_string()))?;
            if references
                .insert(reference.digest.clone(), reference.clone())
                .is_some_and(|existing| existing != *reference)
            {
                return Err(TraceIngestError::Attachment(format!(
                    "digest {} has conflicting reference metadata",
                    reference.digest
                )));
            }
        }
    }
    Ok(references)
}

fn validate_attachments(
    references: &BTreeMap<String, BlobReferenceV1>,
    supplied: Vec<BlobAttachmentV1>,
) -> Result<Vec<BlobAttachmentV1>, TraceIngestError> {
    let mut attachments = BTreeMap::new();
    for attachment in supplied {
        attachment
            .validate()
            .map_err(|error| TraceIngestError::Attachment(error.to_string()))?;
        let digest = attachment.blob.digest.clone();
        if attachments.insert(digest.clone(), attachment).is_some() {
            return Err(TraceIngestError::Attachment(format!(
                "duplicate attachment {digest}"
            )));
        }
    }
    for (digest, reference) in references {
        let attachment = attachments
            .get(digest)
            .ok_or_else(|| TraceIngestError::Attachment(format!("missing attachment {digest}")))?;
        if &attachment.blob != reference {
            return Err(TraceIngestError::Attachment(format!(
                "attachment {digest} metadata differs from its event reference"
            )));
        }
    }
    if let Some(digest) = attachments
        .keys()
        .find(|digest| !references.contains_key(*digest))
    {
        return Err(TraceIngestError::Attachment(format!(
            "unreferenced attachment {digest}"
        )));
    }
    Ok(attachments.into_values().collect())
}
