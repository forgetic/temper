// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The per-event service the emit site hard-codes in its `target:` literal
/// must match the `Service` whose `as_str()` it writes into `service=`. We
/// can't read the literal back at runtime, so this guards the *mapping* the
/// design fixes (§2): the spec assigns each event to exactly one plane.
#[test]
fn event_service_mapping_is_the_spec_assignment() {
    // (Event, Service) pairs straight from §2's "Example events" column.
    let mapping = [
        (Event::IssueOpened, Service::Trigger),
        (Event::WakeReceived, Service::Trigger),
        (Event::CiCompleted, Service::Trigger),
        (Event::LeaseClaimed, Service::Worker),
        (Event::LeaseReleased, Service::Worker),
        (Event::LeaseLost, Service::Worker),
        (Event::ForgeContextRead, Service::Engine),
        (Event::RoleSaturated, Service::Worker),
        (Event::AgentStarted, Service::Agent),
        (Event::AgentFinished, Service::Agent),
        (Event::AgentToolConfigured, Service::Agent),
        (Event::AgentToolExposed, Service::Agent),
        (Event::AgentToolHidden, Service::Agent),
        (Event::McpServerStarted, Service::Agent),
        (Event::McpToolCalled, Service::Agent),
        (Event::McpToolResult, Service::Agent),
        (Event::CodebaseMemoryDiscoveryCompleted, Service::Agent),
        (
            Event::CodebaseMemoryMaintenanceDiscoveryCompleted,
            Service::Worker,
        ),
        (Event::CodebaseMemoryIdentitySelected, Service::Agent),
        (Event::CodebaseMemoryIndexLifecycle, Service::Agent),
        (Event::CodebaseMemoryReadinessWait, Service::Agent),
        (Event::CodebaseMemoryRetentionCompleted, Service::Worker),
        (Event::WorkspaceDiffProduced, Service::Worker),
        (Event::ModelTurnRetrying, Service::Agent),
        (Event::ModelSessionRotated, Service::Worker),
        (Event::ModelProviderDeferred, Service::Engine),
        (Event::ModelProviderWake, Service::Engine),
        (Event::ModelRecoveryCleared, Service::Engine),
        (Event::ModelFailureParked, Service::Engine),
        (Event::TransitionApplied, Service::Engine),
        (Event::ValidationOutcome, Service::Engine),
        (Event::QueueEntered, Service::Engine),
        (Event::GateEvaluated, Service::Engine),
        (Event::PrOpened, Service::Engine),
        (Event::PrUpdated, Service::Engine),
        (Event::PrMerged, Service::Engine),
        (Event::ItemResolved, Service::Engine),
    ];
    // Every event in the catalog is mapped exactly once.
    assert_eq!(mapping.len(), Event::ALL.len());
    for event in Event::ALL {
        assert_eq!(
            mapping.iter().filter(|(e, _)| *e == event).count(),
            1,
            "{event:?} is not mapped to exactly one service"
        );
    }
}
