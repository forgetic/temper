//! Canonical agent activity normalization and non-failing projections.
//!
//! `temper-agent-core::AgentEvent` remains the internal machine protocol. This
//! module owns two independent consumers: the canonical optional-activity
//! normalizer and the always-on, content-free lifecycle sink. Usage accounting,
//! operational tracing, and the optional child-to-worker activity channel
//! consume the normalized activity stream; worker liveness never does.

mod containment;
mod lifecycle;
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

use lifecycle::CompositeEventSink;
pub use lifecycle::{AgentCancellationLatch, AgentLifecycleReporter};

/// Activity and independent correctness-lifecycle settings for one coding-agent
/// invocation.
#[derive(Clone, Default)]
pub struct AgentActivityConfig {
    pub policy: AgentActivityCapturePolicyV1,
    pub address: Option<String>,
    /// Always-on first-party lifecycle endpoint. This is consumed regardless
    /// of `policy.capture` and never shares activity storage or queues.
    pub lifecycle_address: Option<String>,
    /// In-process equivalent of `lifecycle_address` used by the standalone
    /// worker and deterministic fakes.
    pub lifecycle_reporter: Option<AgentLifecycleReporter>,
    /// Cooperative cancellation bridge installed into the core run control.
    pub cancellation: AgentCancellationLatch,
}

impl std::fmt::Debug for AgentActivityConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentActivityConfig")
            .field("policy", &self.policy)
            .field("address", &self.address)
            .field("lifecycle_address", &self.lifecycle_address)
            .field(
                "lifecycle_reporter",
                &self.lifecycle_reporter.as_ref().map(|_| "<reporter>"),
            )
            .field("cancellation", &"<latch>")
            .finish()
    }
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
    lifecycle_projection: Option<Arc<dyn lifecycle::LifecycleProjection>>,
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
        let lifecycle_projection = lifecycle::projection(
            config.lifecycle_address.as_deref(),
            config.lifecycle_reporter,
            config.cancellation,
        );
        Self {
            policy: config.policy,
            clock: Arc::new(SystemActivityClock::new()),
            projections: Arc::new(ProjectionSet { projections }),
            lifecycle_projection,
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
            lifecycle_projection: None,
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

    /// Observer for managed bash and MCP owners sharing this run's always-on
    /// lifecycle carrier. The worker-owned endpoint supplies assignment
    /// identity after validating each content-free frame.
    pub fn containment_observer(
        &self,
        scope_id: &str,
    ) -> Option<Arc<dyn temper_agent_core::CleanupObserver>> {
        self.lifecycle_projection.as_ref().map(|projection| {
            Arc::new(containment::LifecycleCleanupObserver::new(
                Arc::clone(projection),
                temper_protocol_agent::AgentLifecycleScopeV1 {
                    id: scope_id.to_string(),
                    parent_id: None,
                },
            )) as Arc<dyn temper_agent_core::CleanupObserver>
        })
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
        let activity = Arc::new(NormalizingEventSink::new(
            scope.clone(),
            model.clone(),
            display_name,
            self.policy.clone(),
            Arc::clone(&self.clock),
            Arc::clone(&self.projections),
        ));
        let lifecycle = self.lifecycle_projection.as_ref().map(|projection| {
            Arc::new(lifecycle::LifecycleEventSink::new(
                temper_protocol_agent::AgentLifecycleScopeV1 {
                    id: scope.id,
                    parent_id: scope.parent_id,
                },
                Arc::clone(&self.clock),
                Arc::clone(projection),
            ))
        });
        let sink = Arc::new(CompositeEventSink {
            activity,
            lifecycle,
        });
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
