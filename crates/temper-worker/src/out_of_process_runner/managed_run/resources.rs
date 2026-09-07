//! Joined resources and cleanup proof for one agent attempt.
use super::*;
use crate::executor::ResourceJoinReport;
use crate::trace::ActivityEndpoint;

/// Every blocking or threaded resource owned by one attempt. The explicit
/// cancellation path drives this owner to `finish`; Drop is only the abrupt
/// component-loss hard-kill fallback.
pub(super) struct RunResources {
    pub(super) job_id: String,
    pub(super) fence: AttemptFence,
    pub(super) accepted_submit: AcceptedSubmitProofStore,
    pub(super) process: Option<ManagedAgentProcess>,
    pub(super) lifecycle: Option<lifecycle::LifecycleEndpoint>,
    pub(super) activity: Option<ActivityEndpoint>,
    pub(super) trace: Option<TraceRun>,
    pub(super) submit: Option<LocalServer>,
    pub(super) forge: Option<LocalServer>,
    pub(super) finished: bool,
}

impl RunResources {
    pub(super) fn process_mut(&mut self) -> &mut ManagedAgentProcess {
        self.process
            .as_mut()
            .expect("run resources always own a process until quiescence")
    }

    pub(super) fn finish(
        mut self,
        mut result: SupervisorResult,
        cancelled: bool,
        lifecycle_cancellation: ResourceJoinStatus,
    ) -> SupervisorResult {
        result.quiesced.cleanup.resources.process_supervisor = if self
            .process
            .as_mut()
            .is_some_and(ManagedAgentProcess::join_completed)
        {
            ResourceJoinStatus::Joined
        } else {
            ResourceJoinStatus::Failed("agent supervisor thread panicked".to_string())
        };
        self.process.take();
        self.stop_endpoints(&mut result.quiesced.cleanup.resources);
        result.quiesced.cleanup.resources.lifecycle_cancellation = lifecycle_cancellation;
        if cancelled {
            self.finish_cancelled_activity();
            // Clear again after joining accepted handlers. A submit gate that
            // was already running when the fence closed cannot leave proof.
            self.accepted_submit.clear();
        }
        self.finished = true;
        emit_quiesced(&self.job_id, &result.quiesced, cancelled);
        result
    }

    fn finish_cancelled_activity(&self) {
        let Some(trace) = self.trace.as_ref() else {
            return;
        };
        match trace.finish_cancelled() {
            Ok(_) | Err(crate::trace::TraceError::AlreadyTerminal) => {}
            Err(error) => tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.terminal_failed",
                run_id = trace.run_id(),
                job_id = self.job_id,
                %error,
                "worker could not persist synthetic cancelled terminal activity"
            ),
        }
    }

    fn stop_endpoints(&mut self, report: &mut ResourceJoinReport) {
        if let Some(server) = self.submit.take() {
            report.submit_endpoint = join_status(server.stop(), "submit endpoint");
        }
        if let Some(server) = self.forge.take() {
            report.forge_endpoint = join_status(server.stop(), "Forge endpoint");
        }
        if let Some(endpoint) = self.activity.take() {
            report.activity_endpoint = join_status(endpoint.stop(), "activity endpoint");
        }
        if let Some(endpoint) = self.lifecycle.take() {
            report.lifecycle_endpoint = join_status(endpoint.stop(), "lifecycle endpoint");
        }
    }
}

impl Drop for RunResources {
    fn drop(&mut self) {
        if self.finished || self.process.is_none() {
            return;
        }

        // Abrupt owner loss is a last-resort safety path. Watchdog
        // cancellation stays in the async run loop below and never waits for
        // the process supervisor from Drop.
        self.fence.close();
        self.accepted_submit.clear();
        self.process.take();
        let mut ignored = ResourceJoinReport::no_process();
        self.stop_endpoints(&mut ignored);
        self.finish_cancelled_activity();
        self.accepted_submit.clear();
        self.finished = true;
    }
}

pub(super) fn join_status(joined: bool, resource: &str) -> ResourceJoinStatus {
    if joined {
        ResourceJoinStatus::Joined
    } else {
        ResourceJoinStatus::Failed(format!("{resource} thread panicked"))
    }
}

fn emit_quiesced(job_id: &str, outcome: &JobQuiesced, cancelled: bool) {
    let cleanup = &outcome.cleanup;
    let report = &cleanup.containment;
    let recovered = !report.observed_survivors().is_empty()
        || report.omitted_survivors() > 0
        || !matches!(
            report.disposition(),
            temper_process_containment::CleanupDisposition::AlreadyEmpty
        );
    if cancelled || recovered || !cleanup.proves_quiescence() {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "worker.job.quiesced",
            job_id,
            cancellation = ?cleanup.cancellation,
            backend = ?report.backend(),
            root = report.root().value(),
            disposition = ?report.disposition(),
            resources = ?cleanup.resources,
            "agent run cleanup recovered descendants or followed cancellation"
        );
    } else {
        tracing::debug!(
            target: "temper::worker",
            service = "worker",
            event = "worker.job.quiesced",
            job_id,
            cancellation = ?cleanup.cancellation,
            backend = ?report.backend(),
            root = report.root().value(),
            disposition = ?report.disposition(),
            resources = ?cleanup.resources,
            "agent run completed with recursive emptiness and resource joins proven"
        );
    }
}
