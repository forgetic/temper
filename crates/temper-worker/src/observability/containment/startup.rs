use std::sync::Once;

use serde::Serialize;

use super::*;

/// Build and deliver startup capability and stale-cgroup evidence through an
/// injected observer.
pub fn observe_startup_containment_capability(
    worker_id: &str,
    observer: &dyn ContainmentEventObserver,
) {
    let (diagnostic, scavenge) = startup_diagnostic(worker_id);
    observer.observe(&ContainmentEvent::startup(worker_id, &diagnostic));
    if let Some(scavenge) = scavenge {
        observer.observe(&scavenge);
    }
}

static STARTUP_DIAGNOSTIC: Once = Once::new();

pub(crate) fn emit_startup_containment_capability_once(worker_id: &str) {
    STARTUP_DIAGNOSTIC.call_once(|| {
        observe_startup_containment_capability(worker_id, &TracingContainmentEventObserver);
    });
}

#[derive(Serialize)]
struct RetainedScavengeDiagnostic {
    path: String,
    diagnostic: String,
}

pub(super) fn startup_scavenge_from_parts<'a>(
    worker_id: &str,
    removed_count: usize,
    retained: impl IntoIterator<Item = (&'a std::path::Path, &'a str)>,
    omitted: usize,
) -> Option<ContainmentEvent> {
    let retained = retained
        .into_iter()
        .map(|(path, diagnostic)| RetainedScavengeDiagnostic {
            path: bounded_diagnostic(&path.to_string_lossy(), MAX_EVENT_ROOT_BYTES),
            diagnostic: bounded_diagnostic(diagnostic, MAX_EVENT_REASON_BYTES),
        })
        .collect::<Vec<_>>();
    if removed_count == 0 && retained.is_empty() && omitted == 0 {
        return None;
    }
    let retained_count = retained.len();
    let omitted_diagnostics =
        omitted.saturating_add(retained_count.saturating_sub(MAX_EVENT_SURVIVORS));
    let retained_diagnostics = serde_json::to_string(
        &retained
            .into_iter()
            .take(MAX_EVENT_SURVIVORS)
            .collect::<Vec<_>>(),
    )
    .expect("bounded stale-cgroup diagnostics serialize");
    Some(ContainmentEvent::StartupScavenge(
        ContainmentStartupScavenge {
            worker_id: bounded(worker_id, MAX_EVENT_IDENTIFIER_BYTES),
            removed_count,
            retained_count,
            retained_diagnostics,
            omitted_diagnostics,
        },
    ))
}

#[cfg(target_os = "linux")]
fn startup_diagnostic(
    worker_id: &str,
) -> (ContainmentCapabilityDiagnostic, Option<ContainmentEvent>) {
    use temper_process_containment::{CgroupV2BackendFactory, CgroupV2FactoryConfig};

    let config = CgroupV2FactoryConfig::new("startup", "capability")
        .expect("static startup cgroup identity is valid");
    let factory = CgroupV2BackendFactory::system(config);
    let stale_cleanup = factory.scavenge_stale();
    let capability = factory.capability();
    let selected = if capability.delegation_available() {
        ContainmentBackendKind::LinuxCgroupV2
    } else {
        ContainmentBackendKind::LinuxSupervisor
    };
    let diagnostic = ContainmentCapabilityDiagnostic::new(
        capability
            .unified_mount()
            .map(|path| path.to_string_lossy().into_owned()),
        capability.delegation(),
        capability.writable_subtree(),
        capability.cgroup_kill(),
        capability.pidfd(),
        selected,
        (selected == ContainmentBackendKind::LinuxSupervisor).then(|| {
            capability
                .diagnostic()
                .unwrap_or("delegated cgroup-v2 capability requirements were not met")
                .to_string()
        }),
    );
    let scavenge = startup_scavenge_from_parts(
        worker_id,
        stale_cleanup.removed().len(),
        stale_cleanup
            .retained()
            .iter()
            .map(|entry| (entry.path(), entry.diagnostic())),
        stale_cleanup.omitted(),
    );
    (diagnostic, scavenge)
}

#[cfg(windows)]
fn startup_diagnostic(
    _worker_id: &str,
) -> (ContainmentCapabilityDiagnostic, Option<ContainmentEvent>) {
    (
        ContainmentCapabilityDiagnostic::new(
            None,
            false,
            false,
            false,
            false,
            ContainmentBackendKind::WindowsJob,
            None,
        ),
        None,
    )
}

#[cfg(not(any(target_os = "linux", windows)))]
fn startup_diagnostic(
    _worker_id: &str,
) -> (ContainmentCapabilityDiagnostic, Option<ContainmentEvent>) {
    (
        ContainmentCapabilityDiagnostic::new(
            None,
            false,
            false,
            false,
            false,
            ContainmentBackendKind::NoProcess,
            Some("no descendant-complete process containment backend is available".to_string()),
        ),
        None,
    )
}
