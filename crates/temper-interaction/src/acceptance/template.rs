//! Template rendering for acceptance effects.
//!
//! Acceptance effects declare `${...}` template strings rendered against a
//! [`TemplateContext`] built from the request, selected proposal, action, and
//! computed idempotency/marker values.

use crate::{AcceptanceManifest, InteractionError};

use super::{AcceptanceRequest, Proposal};

#[derive(Clone, Copy)]
pub(super) struct TemplateContext<'a> {
    request: AcceptanceRequest<'a>,
    proposal: &'a Proposal,
    action: &'a AcceptanceManifest,
    idempotency_key: Option<&'a str>,
    effect_marker: Option<&'a str>,
}

impl<'a> TemplateContext<'a> {
    pub(super) fn new(
        request: AcceptanceRequest<'a>,
        proposal: &'a Proposal,
        action: &'a AcceptanceManifest,
        idempotency_key: Option<&'a str>,
        effect_marker: Option<&'a str>,
    ) -> Self {
        Self {
            request,
            proposal,
            action,
            idempotency_key,
            effect_marker,
        }
    }
}

pub(super) fn render_template(
    template: &str,
    context: &TemplateContext<'_>,
) -> Result<String, InteractionError> {
    let mut rendered = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(InteractionError::InvalidConfig {
                field: "template",
                message: format!("unterminated template variable in `{template}`"),
            });
        };
        let variable = &after[..end];
        rendered.push_str(&template_value(variable, context)?);
        rest = &after[end + 1..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn template_value(
    variable: &str,
    context: &TemplateContext<'_>,
) -> Result<String, InteractionError> {
    match variable {
        "conversation.id" => Ok(context.request.conversation_id.to_string()),
        "conversation.transcript_url" => Ok(context.request.transcript_url.to_string()),
        "proposal.id" => Ok(context.proposal.id.to_string()),
        "proposal.kind" => Ok(context.proposal.kind.to_string()),
        "proposal.title" => Ok(context.proposal.title.clone()),
        "proposal.summary" => Ok(context.proposal.summary.clone().unwrap_or_default()),
        "human.handle" => Ok(context
            .request
            .requested_by
            .map(|user| user.handle.clone())
            .unwrap_or_default()),
        "acceptance.action_id" => Ok(context.action.id.to_string()),
        "idempotency.key" => Ok(context.idempotency_key.unwrap_or_default().to_string()),
        "effect.marker" => Ok(context.effect_marker.unwrap_or_default().to_string()),
        value if value.starts_with("proposal.payload.") => json_path_string(
            &context.proposal.payload,
            value.trim_start_matches("proposal.payload."),
        ),
        other => Err(InteractionError::InvalidConfig {
            field: "template",
            message: format!("unsupported template variable `{other}`"),
        }),
    }
}

fn json_path_string(value: &serde_json::Value, path: &str) -> Result<String, InteractionError> {
    let mut current = value;
    for segment in path.split('.') {
        current = current
            .get(segment)
            .ok_or_else(|| InteractionError::InvalidConfig {
                field: "template",
                message: format!("proposal payload has no `{path}` field"),
            })?;
    }
    Ok(match current {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(value) => value.clone(),
        other => other.to_string(),
    })
}

pub(super) fn render_non_empty_values(
    values: &[String],
    context: &TemplateContext<'_>,
    field: &'static str,
) -> Result<Vec<String>, InteractionError> {
    values
        .iter()
        .map(|value| {
            let rendered = render_template(value, context)?.trim().to_string();
            if rendered.is_empty() {
                Err(InteractionError::InvalidConfig {
                    field,
                    message: "rendered value must not be empty".into(),
                })
            } else {
                Ok(rendered)
            }
        })
        .collect()
}
