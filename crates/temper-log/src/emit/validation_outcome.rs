// SPDX-License-Identifier: MPL-2.0

//! Typed input and single emit site for `validation.outcome`.

use crate::event::Event;
use crate::service::Service;
use crate::{WorkItemRef, validation_summary_preview};

use super::{human, prefixed};

/// Typed outcome carried by a `validation.outcome` event.
///
/// Plan validation has exactly two successful domain outcomes. Keeping this
/// vocabulary closed prevents arbitrary verdict text from entering the event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationOutcomeKind {
    Validated,
    NeedsFollowup,
}

impl ValidationOutcomeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validated => "validated",
            Self::NeedsFollowup => "needs_followup",
        }
    }
}

/// Inputs for [`emit_validation_outcome`] (`engine` / `validation.outcome`).
///
/// The deliberately narrow field set makes result bodies, details, authored
/// child bodies, tool output, and credentials unrepresentable. `summary` is the
/// sole free-text input and is projected through
/// [`validation_summary_preview`] before it reaches either output.
#[derive(Clone)]
pub struct ValidationOutcome<'a> {
    /// The coordinating plan issue whose validation converged.
    pub item: &'a WorkItemRef,
    /// Closed validation verdict.
    pub outcome: ValidationOutcomeKind,
    /// Workflow role that executed validation (normally `tester`).
    pub workflow_role: &'a str,
    /// Authenticated Forge actor handle (which may differ from the role).
    pub forge_actor_handle: &'a str,
    /// Stable provider user identifier for the authenticated Forge actor.
    pub forge_actor_id: &'a str,
    /// Durable assignment job identifier.
    pub job_id: &'a str,
    /// Routed workflow transition applied for this outcome.
    pub transition: &'a str,
    /// Stable workspace/workflow coordination key.
    pub correlation_key: &'a str,
    /// Number of bounded implementation artifacts in validation scope.
    pub validation_scope_count: usize,
    /// Number of follow-up issues produced (zero for a positive outcome).
    pub follow_up_count: usize,
    /// Agent-provided concise summary; normalized, redacted, and bounded here.
    pub summary: &'a str,
}

/// Emits `engine` / `validation.outcome` after the durable audit and routed
/// transition have converged.
pub fn emit_validation_outcome(ev: ValidationOutcome<'_>) {
    let summary_preview = validation_summary_preview(ev.summary);
    let message = prefixed(
        Service::Engine,
        human::validation_outcome(
            ev.item,
            ev.outcome,
            ev.workflow_role,
            ev.forge_actor_handle,
            ev.forge_actor_id,
            ev.job_id,
            ev.transition,
            ev.correlation_key,
            ev.validation_scope_count,
            ev.follow_up_count,
            &summary_preview,
        ),
    );
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ValidationOutcome.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        outcome = ev.outcome.as_str(),
        role = ev.workflow_role,
        forge.actor.handle = ev.forge_actor_handle,
        forge.actor.id = ev.forge_actor_id,
        job_id = ev.job_id,
        transition = ev.transition,
        correlation.key = ev.correlation_key,
        validation.scope_count = ev.validation_scope_count,
        follow_up.count = ev.follow_up_count,
        summary.preview = %summary_preview,
        "{message}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_vocabulary_matches_verdict_tokens() {
        assert_eq!(ValidationOutcomeKind::Validated.as_str(), "validated");
        assert_eq!(
            ValidationOutcomeKind::NeedsFollowup.as_str(),
            "needs_followup"
        );
    }
}
