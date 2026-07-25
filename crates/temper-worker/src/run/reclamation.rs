use std::time::Duration;

use temper_worker_io::Spawner;

use crate::task_registry::{WorkerComponentTaskKind, WorkerComponentTasks};
use crate::trace::TraceCollector;
use crate::worker_shell::WorkerCancellation;

/// Maximum actionable trace spools inspected synchronously before assignment
/// intake. Sixteen matches the fixed aggregate reservation ceiling, so a fully
/// saturated production spool can admit a new trace after one startup pass.
pub const STARTUP_TRACE_RECLAMATION_RUN_BUDGET: usize = 16;
const BACKGROUND_TRACE_RECLAMATION_RUN_BUDGET: usize = 16;
const BACKGROUND_TRACE_RECLAMATION_YIELD: Duration = Duration::from_millis(100);

pub(super) fn reclaim_activity_traces_at_startup(collector: &TraceCollector) -> bool {
    match collector.reclaim_abandoned_runs(STARTUP_TRACE_RECLAMATION_RUN_BUDGET) {
        Ok(report) => {
            tracing::info!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.startup_recovery",
                terminalized_runs = report.terminalized_runs,
                quarantined_runs = report.quarantined_runs,
                protected_runs = report.protected_runs,
                failed_runs = report.failed_runs,
                remaining_dirty_runs = report.remaining_dirty_runs,
                physical_used_bytes = report.physical_used_bytes,
                logical_reserved_bytes = report.logical_reserved_bytes,
                "worker startup activity recovery: terminalized {}, quarantined {}, protected {}, failed {}, remaining dirty {}, physical used bytes {}, logical reserved bytes {}",
                report.terminalized_runs,
                report.quarantined_runs,
                report.protected_runs,
                report.failed_runs,
                report.remaining_dirty_runs,
                report.physical_used_bytes,
                report.logical_reserved_bytes,
            );
            report.remaining_dirty_runs > 0
        }
        Err(error) => {
            tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.startup_recovery_failed",
                %error,
                "worker startup activity recovery failed; assignment intake will continue"
            );
            true
        }
    }
}

pub(super) fn spawn_background_trace_reclamation<S: Spawner>(
    spawner: S,
    collector: TraceCollector,
    cancellation: WorkerCancellation,
    component_tasks: WorkerComponentTasks,
) {
    let Some(task_guard) = component_tasks.register(WorkerComponentTaskKind::BackgroundComponent)
    else {
        return;
    };
    spawner.spawn_task(async move {
        let _task_guard = task_guard;
        loop {
            if cancellation
                .run(temper_worker_io::sleep_for(
                    BACKGROUND_TRACE_RECLAMATION_YIELD,
                ))
                .await
                .is_none()
            {
                break;
            }
            match collector.reclaim_abandoned_runs(BACKGROUND_TRACE_RECLAMATION_RUN_BUDGET) {
                Ok(report) if report.remaining_dirty_runs == 0 => break,
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.background_recovery_failed",
                    %error,
                    "worker background activity recovery pass failed; product work will continue"
                ),
            }
        }
    });
}
