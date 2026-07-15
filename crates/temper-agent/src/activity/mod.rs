//! Canonical agent activity normalization and non-failing projections.
//!
//! `temper-agent-core::AgentEvent` remains the internal machine protocol. This
//! module is the only production seam that converts those events into the
//! shared typed activity vocabulary; usage accounting, operational tracing, and
//! the optional child-to-worker activity channel all consume that normalized
//! stream.

mod normalizer;
mod transport;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use temper_agent_core::{ModelIdentity, RunObservability};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityChildRecordV1, AgentScopeKindV1, AgentScopeV1,
    CaptureModeV1,
};

use crate::usage::{TracingProjection, UsageTotals};
use normalizer::NormalizingEventSink;
use transport::ActivityClient;

/// Optional activity settings for one coding-agent invocation.
#[derive(Clone, Debug, Default)]
pub struct AgentActivityConfig {
    pub policy: AgentActivityCapturePolicyV1,
    pub address: Option<String>,
}

/// One source-clock observation used to stamp a normalized child frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityTimestamp {
    pub occurred_at: String,
    pub elapsed_ms: u64,
}

/// Injectable source clock for deterministic normalization tests.
pub trait ActivityClock: Send + Sync {
    fn now(&self) -> ActivityTimestamp;
}

struct SystemActivityClock {
    origin: Instant,
}

impl SystemActivityClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl ActivityClock for SystemActivityClock {
    fn now(&self) -> ActivityTimestamp {
        ActivityTimestamp {
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            elapsed_ms: u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// Synchronous projection over one canonical child record. Implementations
/// must not report failures to the agent; the composite additionally catches
/// panics so one broken projection cannot alter the assigned result. Frame-only
/// projections deliberately inspect `record.frame` and ignore attachments.
pub trait ActivityProjection: Send + Sync {
    fn emit(&self, record: &AgentActivityChildRecordV1);
}

pub(super) struct ProjectionSet {
    projections: Vec<Arc<dyn ActivityProjection>>,
}

impl ProjectionSet {
    pub(super) fn emit(&self, record: &AgentActivityChildRecordV1) {
        for projection in &self.projections {
            let _ = catch_unwind(AssertUnwindSafe(|| projection.emit(record)));
        }
    }
}

/// Factory for unique main and nested invocation scopes that share one set of
/// normalized projections and one run source clock.
#[derive(Clone)]
pub struct ScopeFactory {
    policy: AgentActivityCapturePolicyV1,
    clock: Arc<dyn ActivityClock>,
    projections: Arc<ProjectionSet>,
}

/// A core run observer plus the unique scope identity it carries. Callers keep
/// the ID to parent nested scope factories correctly.
pub struct ScopedRunObservability {
    pub scope_id: String,
    pub observability: RunObservability,
}

impl ScopeFactory {
    /// Build the production projection set. Operational tracing and totals are
    /// always active; transport is added only when the worker supplies an
    /// activity address and capture is not off.
    pub fn new(config: AgentActivityConfig, totals: Arc<UsageTotals>) -> Self {
        let mut projections: Vec<Arc<dyn ActivityProjection>> =
            vec![Arc::new(TracingProjection::new(totals))];
        if config.policy.capture != CaptureModeV1::Off {
            if let Some(address) = config.address.as_deref() {
                projections.push(Arc::new(ActivityClient::new(address)));
            }
        }
        Self {
            policy: config.policy,
            clock: Arc::new(SystemActivityClock::new()),
            projections: Arc::new(ProjectionSet { projections }),
        }
    }

    #[cfg(test)]
    fn with_parts(
        policy: AgentActivityCapturePolicyV1,
        clock: Arc<dyn ActivityClock>,
        projections: Vec<Arc<dyn ActivityProjection>>,
    ) -> Self {
        Self {
            policy,
            clock,
            projections: Arc::new(ProjectionSet { projections }),
        }
    }

    /// Mint the top-level invocation scope. IDs are opaque random UUIDs; the
    /// display label is data, never identity.
    pub fn main(
        &self,
        display_name: impl Into<String>,
        model: ModelIdentity,
    ) -> ScopedRunObservability {
        self.scoped(None, AgentScopeKindV1::Main, display_name.into(), model)
    }

    /// Mint one nested invocation scope with an explicit parent. Calling this
    /// method for every tool execution makes concurrent sub-agents distinct.
    pub fn child(
        &self,
        parent_scope_id: impl Into<String>,
        display_name: impl Into<String>,
        model: ModelIdentity,
    ) -> ScopedRunObservability {
        self.scoped(
            Some(parent_scope_id.into()),
            AgentScopeKindV1::SubAgent,
            display_name.into(),
            model,
        )
    }

    fn scoped(
        &self,
        parent_id: Option<String>,
        kind: AgentScopeKindV1,
        display_name: String,
        model: ModelIdentity,
    ) -> ScopedRunObservability {
        let scope = AgentScopeV1 {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            parent_id,
        };
        let scope_id = scope.id.clone();
        let sink = Arc::new(NormalizingEventSink::new(
            scope,
            display_name,
            self.policy.clone(),
            Arc::clone(&self.clock),
            Arc::clone(&self.projections),
        ));
        ScopedRunObservability {
            scope_id,
            observability: RunObservability::new(sink, model),
        }
    }
}

#[cfg(test)]
mod prompt_tests;
#[cfg(test)]
mod tests;
