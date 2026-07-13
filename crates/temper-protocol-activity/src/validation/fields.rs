use crate::{
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentScopeKindV1, AgentScopeV1,
    CapturedContentV1, FailureInfoV1, InlineContentV1, MAX_IDENTIFIER_BYTES,
    MAX_INLINE_CONTENT_BYTES,
};

use super::{ActivityValidationCode, ActivityValidationError, error, validate_blob_reference};

pub(super) fn assignment(
    identity: &AgentAssignmentIdentityV1,
) -> Result<(), ActivityValidationError> {
    identifier(&identity.job_id, "event.assignment.job_id")?;
    identifier(&identity.repository, "event.assignment.repository")?;
    identifier(&identity.artifact_ref, "event.assignment.artifact_ref")?;
    identifier(&identity.role, "event.assignment.role")?;
    identifier(&identity.action, "event.assignment.action")?;
    identifier(
        &identity.correlation_key,
        "event.assignment.correlation_key",
    )
}

pub(super) fn scope(scope_value: &AgentScopeV1, path: &str) -> Result<(), ActivityValidationError> {
    identifier(&scope_value.id, &format!("{path}.id"))?;
    if let Some(parent_id) = &scope_value.parent_id {
        identifier(parent_id, &format!("{path}.parent_id"))?;
        if parent_id == &scope_value.id {
            return Err(error(
                ActivityValidationCode::MalformedScope,
                format!("{path}.parent_id"),
                "a scope cannot be its own parent",
            ));
        }
    }
    match (scope_value.kind, &scope_value.parent_id) {
        (AgentScopeKindV1::Main, Some(_)) => Err(error(
            ActivityValidationCode::MalformedScope,
            format!("{path}.parent_id"),
            "the main scope cannot have a parent",
        )),
        (AgentScopeKindV1::SubAgent, None) => Err(error(
            ActivityValidationCode::MalformedScope,
            format!("{path}.parent_id"),
            "a sub-agent scope must identify its parent",
        )),
        _ => Ok(()),
    }
}

pub(super) fn event(
    event_value: &AgentActivityEventV1,
    turn: Option<u32>,
    path: &str,
) -> Result<(), ActivityValidationError> {
    use AgentActivityEventV1 as Event;
    match event_value {
        Event::RunStarted(_) | Event::RunFinished(_) | Event::Usage(_) => Ok(()),
        Event::ScopeStarted(value) => optional_short_text(
            value.display_name.as_deref(),
            &format!("{path}.data.display_name"),
        ),
        Event::ScopeFinished(_) => Ok(()),
        Event::TurnStarted(_) | Event::TurnFinished(_) if turn.is_none() => Err(error(
            ActivityValidationCode::MissingTurn,
            "event.turn",
            "turn boundary events require turn context",
        )),
        Event::TurnStarted(_) | Event::TurnFinished(_) => Ok(()),
        Event::ModelCallStarted(value) => {
            identifier(&value.call_id, &format!("{path}.data.call_id"))?;
            identifier(&value.provider, &format!("{path}.data.provider"))?;
            identifier(&value.model, &format!("{path}.data.model"))
        }
        Event::ModelCallRetrying(value) => {
            identifier(&value.call_id, &format!("{path}.data.call_id"))?;
            if value.next_attempt == 0 {
                return Err(invalid_event(
                    &format!("{path}.data.next_attempt"),
                    "must be greater than zero",
                ));
            }
            failure(&value.failure, &format!("{path}.data.failure"))
        }
        Event::ModelCallFinished(value) => {
            identifier(&value.call_id, &format!("{path}.data.call_id"))?;
            if value
                .time_to_first_token_ms
                .is_some_and(|first_token| first_token > value.duration_ms)
            {
                return Err(invalid_event(
                    &format!("{path}.data.time_to_first_token_ms"),
                    "cannot exceed model-call duration",
                ));
            }
            Ok(())
        }
        Event::AssistantMessage(value) => {
            identifier(&value.message_id, &format!("{path}.data.message_id"))?;
            captured_content(&value.content, &format!("{path}.data.content"))
        }
        Event::OutputTextDelta(value) | Event::OutputThinkingDelta(value) => {
            inline_content(&value.delta, &format!("{path}.data.delta"))
        }
        Event::ToolStarted(value) => {
            identifier(&value.call_id, &format!("{path}.data.call_id"))?;
            identifier(&value.name, &format!("{path}.data.name"))?;
            optional_content(value.arguments.as_ref(), &format!("{path}.data.arguments"))
        }
        Event::ToolFinished(value) => {
            identifier(&value.call_id, &format!("{path}.data.call_id"))?;
            identifier(&value.name, &format!("{path}.data.name"))?;
            optional_content(value.result.as_ref(), &format!("{path}.data.result"))
        }
        Event::SteeringApplied(value) => optional_content(
            value.instruction.as_ref(),
            &format!("{path}.data.instruction"),
        ),
        Event::TraceGap(value) => {
            if value.dropped_events == 0 || value.kinds.is_empty() {
                return Err(invalid_event(
                    &format!("{path}.data"),
                    "a trace gap requires a non-zero event count and at least one kind",
                ));
            }
            Ok(())
        }
        Event::RunFailed(value) => failure(&value.failure, &format!("{path}.data.failure")),
    }
}

fn optional_content(
    content: Option<&CapturedContentV1>,
    path: &str,
) -> Result<(), ActivityValidationError> {
    content.map_or(Ok(()), |content| captured_content(content, path))
}

fn captured_content(
    content: &CapturedContentV1,
    path: &str,
) -> Result<(), ActivityValidationError> {
    match content {
        CapturedContentV1::Inline(inline) => inline_content(inline, path),
        CapturedContentV1::Blob { blob } => validate_blob_reference(blob),
    }
}

fn inline_content(content: &InlineContentV1, path: &str) -> Result<(), ActivityValidationError> {
    if content.text.is_empty() {
        return Err(error(
            ActivityValidationCode::InvalidEvent,
            format!("{path}.text"),
            "must not be empty",
        ));
    }
    if content.text.len() > MAX_INLINE_CONTENT_BYTES {
        return Err(error(
            ActivityValidationCode::OversizedInlineValue,
            format!("{path}.text"),
            format!("exceeds {MAX_INLINE_CONTENT_BYTES} UTF-8 bytes"),
        ));
    }
    Ok(())
}

fn failure(value: &FailureInfoV1, path: &str) -> Result<(), ActivityValidationError> {
    if value.message.trim().is_empty() {
        return Err(invalid_event(
            &format!("{path}.message"),
            "must not be empty",
        ));
    }
    if value.message.len() > MAX_INLINE_CONTENT_BYTES {
        return Err(error(
            ActivityValidationCode::OversizedInlineValue,
            format!("{path}.message"),
            format!("exceeds {MAX_INLINE_CONTENT_BYTES} UTF-8 bytes"),
        ));
    }
    Ok(())
}

fn optional_short_text(value: Option<&str>, path: &str) -> Result<(), ActivityValidationError> {
    value.map_or(Ok(()), |value| identifier(value, path))
}

pub(super) fn identifier(value: &str, field: &str) -> Result<(), ActivityValidationError> {
    if value.trim().is_empty() {
        return Err(error(
            ActivityValidationCode::EmptyIdentifier,
            field,
            "must not be empty",
        ));
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(error(
            ActivityValidationCode::IdentifierTooLong,
            field,
            format!("exceeds {MAX_IDENTIFIER_BYTES} UTF-8 bytes"),
        ));
    }
    Ok(())
}

pub(super) fn version(value: u32, field: &str) -> Result<(), ActivityValidationError> {
    if value == crate::ACTIVITY_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(error(
            ActivityValidationCode::UnsupportedVersion,
            field,
            format!(
                "unsupported version {value}; expected {}",
                crate::ACTIVITY_PROTOCOL_VERSION
            ),
        ))
    }
}

pub(super) fn timestamp(value: &str, field: &str) -> Result<(), ActivityValidationError> {
    if valid_rfc3339(value) {
        Ok(())
    } else {
        Err(error(
            ActivityValidationCode::InvalidTimestamp,
            field,
            "must be an RFC3339 timestamp with an explicit offset",
        ))
    }
}

fn valid_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !value.is_ascii()
        || bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }
    let Some(year) = digits(bytes, 0, 4) else {
        return false;
    };
    let Some(month) = digits(bytes, 5, 2) else {
        return false;
    };
    let Some(day) = digits(bytes, 8, 2) else {
        return false;
    };
    let Some(hour) = digits(bytes, 11, 2) else {
        return false;
    };
    let Some(minute) = digits(bytes, 14, 2) else {
        return false;
    };
    let Some(second) = digits(bytes, 17, 2) else {
        return false;
    };
    if year == 0
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }

    let mut offset_start = 19;
    if bytes.get(offset_start) == Some(&b'.') {
        offset_start += 1;
        let fractional_start = offset_start;
        while bytes.get(offset_start).is_some_and(u8::is_ascii_digit) {
            offset_start += 1;
        }
        if offset_start == fractional_start {
            return false;
        }
    }
    match bytes.get(offset_start) {
        Some(b'Z' | b'z') => offset_start + 1 == bytes.len(),
        Some(b'+' | b'-') => {
            bytes.len() == offset_start + 6
                && bytes.get(offset_start + 3) == Some(&b':')
                && digits(bytes, offset_start + 1, 2).is_some_and(|hours| hours <= 23)
                && digits(bytes, offset_start + 4, 2).is_some_and(|minutes| minutes <= 59)
        }
        _ => false,
    }
}

fn digits(bytes: &[u8], start: usize, length: usize) -> Option<u32> {
    let slice = bytes.get(start..start + length)?;
    if !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        slice
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')),
    )
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn invalid_event(field: &str, detail: impl Into<String>) -> ActivityValidationError {
    error(ActivityValidationCode::InvalidEvent, field, detail)
}
