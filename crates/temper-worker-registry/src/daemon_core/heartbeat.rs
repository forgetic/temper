// SPDX-License-Identifier: MPL-2.0

//! Exact-attempt heartbeat validation and latest observability-report storage.

use temper_protocol_worker::{ErrorCode, Heartbeat, WorkerAuth, WorkerProtocolMessage};

use super::{DaemonCore, HeartbeatRecovery, WorkerAuthError, error_response};

impl DaemonCore {
    /// Authenticates and applies a heartbeat while exposing exact durable-job
    /// matches to the daemon lease handler.
    pub fn handle_authenticated_heartbeat(
        &mut self,
        heartbeat: Heartbeat,
        auth: Option<&WorkerAuth>,
    ) -> Result<(Option<WorkerProtocolMessage>, HeartbeatRecovery), WorkerAuthError> {
        self.authenticate_registered_worker(
            &heartbeat.worker_id,
            heartbeat.worker_pool.as_deref(),
            auth,
        )?;
        Ok(self.handle_heartbeat(heartbeat))
    }

    fn handle_heartbeat(
        &mut self,
        heartbeat: Heartbeat,
    ) -> (Option<WorkerProtocolMessage>, HeartbeatRecovery) {
        if self
            .coordinator
            .registry_mut()
            .heartbeat(&heartbeat.worker_id)
            .is_err()
        {
            return (
                Some(error_response(
                    ErrorCode::UnknownWorker,
                    "unknown worker",
                    None,
                )),
                HeartbeatRecovery::default(),
            );
        }

        let mut recovery = HeartbeatRecovery::default();
        for reported in heartbeat.jobs {
            let job_id = reported.job_id.clone();
            if self.coordinator.assigned_worker(&job_id) == Some(heartbeat.worker_id.as_str())
                && self.assignment_attempts.get(&job_id).map(String::as_str)
                    == reported.attempt_id.as_deref()
            {
                let _ = self
                    .coordinator
                    .registry_mut()
                    .report_job(&heartbeat.worker_id, reported.clone());
                recovery.matched_reports.push(reported);
                continue;
            }

            let Some(staged) = self.staged_recovery.get(&job_id).cloned() else {
                recovery.rejected_reports.push(reported);
                continue;
            };
            if staged.worker_id != heartbeat.worker_id
                || staged.attempt_id.as_deref() != reported.attempt_id.as_deref()
            {
                recovery.rejected_reports.push(reported);
                continue;
            }
            match self
                .coordinator
                .restore_assignment(&staged.worker_id, staged.item)
            {
                Ok(_) => {
                    self.staged_recovery.remove(&job_id);
                    if let Some(attempt_id) = staged.attempt_id {
                        self.assignment_attempts.insert(job_id, attempt_id);
                    }
                    let _ = self
                        .coordinator
                        .registry_mut()
                        .report_job(&heartbeat.worker_id, reported.clone());
                    recovery.matched_reports.push(reported);
                }
                Err(_) => recovery.rejected_reports.push(reported),
            }
        }
        (None, recovery)
    }
}
