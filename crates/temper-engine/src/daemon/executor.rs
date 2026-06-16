// SPDX-License-Identifier: MPL-2.0

//! The daemon's imperative shell: performs each machine request on the engine
//! runtime and feeds the resulting completions back into the queue.

use std::sync::Arc;

use temper_engine_io::{CqSender, Executor as EngineExecutor, Spawner, arm_timer};

use crate::applier::ResultApplier;

use super::WakeScanner;
use super::machine::{DaemonCompletion, DaemonMachine, DaemonRequest};

pub(super) struct DaemonExecutor {
    pub(super) spawner: Arc<dyn Spawner>,
    pub(super) cq: CqSender<DaemonCompletion>,
    pub(super) applier: Arc<dyn ResultApplier>,
    pub(super) scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>>,
}

impl EngineExecutor<DaemonMachine> for DaemonExecutor {
    fn execute(&self, request: DaemonRequest) {
        match request {
            DaemonRequest::Respond {
                responder,
                response,
            } => responder.respond(response),
            DaemonRequest::StartPollTimer { id, delay } => {
                arm_timer(&*self.spawner, &self.cq, delay, move || {
                    DaemonCompletion::PollDeadline { id }
                });
            }
            DaemonRequest::RunApply { job, result } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                let job_id = job.job_id.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    applier.apply(job, result).await;
                    let _ = cq.send(DaemonCompletion::ApplyFinished { job_id });
                });
            }
            DaemonRequest::RunProgressApply { job, progress } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    applier.apply_progress(job, progress).await;
                    let _ = cq.send(DaemonCompletion::ProgressApplyFinished);
                });
            }
            DaemonRequest::RunWakeScan { token, hint } => {
                let scanner = self.scanner_slot.lock().expect("scanner slot").clone();
                let cq = self.cq.clone();
                match scanner {
                    Some(scanner) => {
                        self.spawner.spawn_with_cx(move |_cx| async move {
                            scanner.scan(hint).await;
                            let _ = cq.send(DaemonCompletion::WakeScanFinished { token });
                        });
                    }
                    None => {
                        let _ = cq.send(DaemonCompletion::WakeScanFinished { token });
                    }
                }
            }
            DaemonRequest::RoleSaturated {
                role,
                concurrency,
                waiting,
            } => {
                temper_log::emit::emit_role_saturated(temper_log::emit::RoleSaturated {
                    role: &role,
                    concurrency,
                    waiting: &waiting,
                });
            }
            DaemonRequest::Log(line) => tracing::info!("{line}"),
            #[cfg(test)]
            DaemonRequest::QueuedJobsReply(reply, jobs) => reply.send(jobs),
        }
    }
}
