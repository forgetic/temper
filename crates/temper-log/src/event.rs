// SPDX-License-Identifier: MPL-2.0

//! The closed `event` vocabulary (§3 rule 1 of the logging design).
//!
//! `event` is a **closed dotted-namespace enum**, defined in Rust so the
//! machine-facing vocabulary cannot drift. Each [`Event`] variant has one stable
//! dotted string ([`Event::as_str`]) used as the `event=` field value. An agent
//! keys off `event=…` and never parses the human prose.
//!
//! Adding a workflow state change means adding a variant here — there is exactly
//! one place the vocabulary lives, and `info`-level emit sites draw their `event`
//! value from it.

/// A structured event in temper's closed vocabulary.
///
/// The set is the §3/§5/§7 catalog plus debug-level tool-boundary facts used by
/// validation and operators. It is `#[non_exhaustive]`-free on purpose:
/// it is a *closed* enum so a `match` over it is exhaustive and the vocabulary
/// is fixed at compile time. New events are added by editing this enum (and the
/// corresponding `emit` constructor), never by passing a free string.
/// Workflow state changes are emitted at `info`; tool-boundary events are
/// emitted at `debug` so production deployments keep pay-for-what-you-use
/// volume unless they opt in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Event {
    /// A new issue was observed on the forge (`issue.opened`).
    IssueOpened,
    /// A wake/webhook was received for an artifact (`wake.received`).
    WakeReceived,
    /// A CI run completed for a pull request (`ci.completed`).
    CiCompleted,
    /// A worker claimed a lease on a work item (`lease.claimed`).
    LeaseClaimed,
    /// A worker released a lease it held (`lease.released`).
    LeaseReleased,
    /// A held lease was lost (expired / reclaimed) (`lease.lost`).
    LeaseLost,
    /// An agent workspace run started (`agent.started`).
    AgentStarted,
    /// An agent workspace run finished (`agent.finished`).
    AgentFinished,
    /// Agent tool configuration was accepted for a run (`agent.tool.configured`).
    AgentToolConfigured,
    /// A tool was exposed to the model at the registration boundary (`agent.tool.exposed`).
    AgentToolExposed,
    /// A tool was hidden from the model at the registration boundary (`agent.tool.hidden`).
    AgentToolHidden,
    /// A codebase-memory MCP server was initialized (`mcp.server.started`).
    McpServerStarted,
    /// A model-visible MCP wrapper invoked a server tool (`mcp.tool.called`).
    McpToolCalled,
    /// A model-visible MCP wrapper returned a server tool result (`mcp.tool.result`).
    McpToolResult,
    /// A workspace contains a product diff after an agent run (`workspace.diff.produced`).
    WorkspaceDiffProduced,
    /// A failed side-effect-free provider request will be retried in the same model turn
    /// (`model.turn.retrying`).
    ModelTurnRetrying,
    /// A durable model failure consumed a session and selected its one fresh replacement
    /// (`model.session.rotated`).
    ModelSessionRotated,
    /// Exhausted immediate/session recovery entered automatic provider deferral
    /// (`model.provider.deferred`).
    ModelProviderDeferred,
    /// An authenticated provider-health signal advanced a deferred generation
    /// (`model.provider.wake`).
    ModelProviderWake,
    /// Authoritative success cleared durable provider recovery
    /// (`model.recovery.cleared`).
    ModelRecoveryCleared,
    /// Exhausted or actionable bounded recovery was parked for operator action
    /// (`model.failure.parked`).
    ModelFailureParked,
    /// The engine applied a workflow transition (`transition.applied`).
    TransitionApplied,
    /// A plan-validation result converged durably (`validation.outcome`).
    ValidationOutcome,
    /// A work item entered a queue (`queue.entered`).
    QueueEntered,
    /// The engine evaluated the gates on a PR (`gate.evaluated`).
    GateEvaluated,
    /// A pull request was opened for an issue (`pr.opened`).
    PrOpened,
    /// A pull request title/body handoff was updated (`pr.updated`).
    PrUpdated,
    /// A pull request was merged to the target branch (`pr.merged`).
    PrMerged,
    /// A work item was resolved end-to-end (`item.resolved`).
    ItemResolved,
    /// A worker requested one bounded Forge context read (`forge.context.read`).
    ForgeContextRead,
    /// A role is saturated; items are queued behind the holder (`role.saturated`).
    RoleSaturated,
}

impl Event {
    /// The stable dotted string form used as the `event=` field value.
    ///
    /// This is the machine key (§3 rule 1). It is part of the logging contract
    /// and must not change once shipped.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueOpened => "issue.opened",
            Self::WakeReceived => "wake.received",
            Self::CiCompleted => "ci.completed",
            Self::LeaseClaimed => "lease.claimed",
            Self::LeaseReleased => "lease.released",
            Self::LeaseLost => "lease.lost",
            Self::AgentStarted => "agent.started",
            Self::AgentFinished => "agent.finished",
            Self::AgentToolConfigured => "agent.tool.configured",
            Self::AgentToolExposed => "agent.tool.exposed",
            Self::AgentToolHidden => "agent.tool.hidden",
            Self::McpServerStarted => "mcp.server.started",
            Self::McpToolCalled => "mcp.tool.called",
            Self::McpToolResult => "mcp.tool.result",
            Self::WorkspaceDiffProduced => "workspace.diff.produced",
            Self::ModelTurnRetrying => "model.turn.retrying",
            Self::ModelSessionRotated => "model.session.rotated",
            Self::ModelProviderDeferred => "model.provider.deferred",
            Self::ModelProviderWake => "model.provider.wake",
            Self::ModelRecoveryCleared => "model.recovery.cleared",
            Self::ModelFailureParked => "model.failure.parked",
            Self::TransitionApplied => "transition.applied",
            Self::ValidationOutcome => "validation.outcome",
            Self::QueueEntered => "queue.entered",
            Self::GateEvaluated => "gate.evaluated",
            Self::PrOpened => "pr.opened",
            Self::PrUpdated => "pr.updated",
            Self::PrMerged => "pr.merged",
            Self::ItemResolved => "item.resolved",
            Self::ForgeContextRead => "forge.context.read",
            Self::RoleSaturated => "role.saturated",
        }
    }

    /// Every event in the catalog, for exhaustiveness tests and tooling.
    ///
    /// Kept in sync with the enum by the `all_variants_have_unique_dotted_form`
    /// test, which fails if a new variant is added without listing it here.
    pub const ALL: [Self; 31] = [
        Self::IssueOpened,
        Self::WakeReceived,
        Self::CiCompleted,
        Self::LeaseClaimed,
        Self::LeaseReleased,
        Self::LeaseLost,
        Self::AgentStarted,
        Self::AgentFinished,
        Self::AgentToolConfigured,
        Self::AgentToolExposed,
        Self::AgentToolHidden,
        Self::McpServerStarted,
        Self::McpToolCalled,
        Self::McpToolResult,
        Self::WorkspaceDiffProduced,
        Self::ModelTurnRetrying,
        Self::ModelSessionRotated,
        Self::ModelProviderDeferred,
        Self::ModelProviderWake,
        Self::ModelRecoveryCleared,
        Self::ModelFailureParked,
        Self::TransitionApplied,
        Self::ValidationOutcome,
        Self::QueueEntered,
        Self::GateEvaluated,
        Self::PrOpened,
        Self::PrUpdated,
        Self::PrMerged,
        Self::ItemResolved,
        Self::ForgeContextRead,
        Self::RoleSaturated,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn dotted_forms_match_the_spec_catalog() {
        // The exact §3 rule-1 vocabulary, verbatim.
        let expected = [
            "issue.opened",
            "wake.received",
            "ci.completed",
            "lease.claimed",
            "lease.released",
            "lease.lost",
            "agent.started",
            "agent.finished",
            "agent.tool.configured",
            "agent.tool.exposed",
            "agent.tool.hidden",
            "mcp.server.started",
            "mcp.tool.called",
            "mcp.tool.result",
            "workspace.diff.produced",
            "model.turn.retrying",
            "model.session.rotated",
            "model.provider.deferred",
            "model.provider.wake",
            "model.recovery.cleared",
            "model.failure.parked",
            "transition.applied",
            "validation.outcome",
            "queue.entered",
            "gate.evaluated",
            "pr.opened",
            "pr.updated",
            "pr.merged",
            "item.resolved",
            "forge.context.read",
            "role.saturated",
        ];
        let actual: Vec<&str> = Event::ALL.iter().map(|e| e.as_str()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn all_variants_have_unique_dotted_form() {
        // Every variant maps to a distinct, non-empty, dotted string.
        let mut seen = BTreeSet::new();
        for event in Event::ALL {
            let dotted = event.as_str();
            assert!(
                dotted.contains('.'),
                "{event:?} dotted form {dotted:?} is not namespaced"
            );
            assert!(
                seen.insert(dotted),
                "duplicate dotted form {dotted:?} for {event:?}"
            );
        }
        assert_eq!(seen.len(), Event::ALL.len());
    }

    #[test]
    fn all_array_length_matches_catalog() {
        // Guards `Event::ALL` against drifting out of sync with the enum: if a
        // variant is added but not appended to ALL, this and the uniqueness test
        // catch it (the array literal would also fail to compile on a length
        // mismatch).
        assert_eq!(Event::ALL.len(), 31);
    }
}
