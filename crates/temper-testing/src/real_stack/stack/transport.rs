use std::future::Future;
use std::sync::{Arc, Mutex};

use skein::cx::Cx;
use temper_engine::Daemon;
use temper_protocol_worker::{JobResult, ResultStatus, WorkerAuth, WorkerProtocolMessage};
use temper_worker::Transport;

use super::super::pause::{PauseHooks, PausePoint};
use super::{PublishedReleases, PublishedResults};

/// Replaceable endpoint resolved on every message, like an external reconnect.
pub struct DaemonRouter {
    daemon: Mutex<Arc<Daemon>>,
}

impl DaemonRouter {
    pub(crate) fn new(daemon: Arc<Daemon>) -> Self {
        Self {
            daemon: Mutex::new(daemon),
        }
    }

    pub(crate) fn replace(&self, daemon: Arc<Daemon>) {
        *self.daemon.lock().expect("daemon router lock") = daemon;
    }

    pub(crate) fn current(&self) -> Arc<Daemon> {
        self.daemon.lock().expect("daemon router lock").clone()
    }
}

/// Replaceable-daemon transport that records worker results for assertions.
pub struct ResultTappingTransport {
    router: Arc<DaemonRouter>,
    result_tx: temper_engine_io::CqSender<JobResult>,
    published_results: PublishedResults,
    published_releases: PublishedReleases,
    hooks: PauseHooks,
}

impl ResultTappingTransport {
    pub(super) fn new(
        router: Arc<DaemonRouter>,
        result_tx: temper_engine_io::CqSender<JobResult>,
        published_results: PublishedResults,
        published_releases: PublishedReleases,
        hooks: PauseHooks,
    ) -> Self {
        Self {
            router,
            result_tx,
            published_results,
            published_releases,
            hooks,
        }
    }
}

impl Transport for ResultTappingTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let result_tx = self.result_tx.clone();
        let published_results = Arc::clone(&self.published_results);
        let published_releases = Arc::clone(&self.published_releases);
        let hooks = self.hooks.clone();
        async move {
            let reports_jobs = matches!(
                &message,
                WorkerProtocolMessage::Heartbeat(heartbeat) if !heartbeat.jobs.is_empty()
            );
            let terminal_activity = matches!(
                &message,
                WorkerProtocolMessage::ActivityBatch(activity)
                    if activity.batch.events.iter().any(|event| event.event.is_terminal())
            );
            if reports_jobs {
                hooks.reach(PausePoint::WorkerHeartbeatReportingJob).await;
            }
            if terminal_activity {
                hooks.reach(PausePoint::WorkerTerminalTraceForwarding).await;
            }
            // Resolve after the pause so parked requests reconnect to a new daemon.
            let recorded = match &message {
                WorkerProtocolMessage::Result(result) => Some(result.clone()),
                _ => None,
            };
            let first_publication = recorded.as_ref().is_some_and(|result| {
                let key = (result.job_id.clone(), result.attempt_id.clone());
                let mut published = published_results.lock().expect("published result lock");
                match published.entry(key) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(result.clone());
                        true
                    }
                    std::collections::btree_map::Entry::Occupied(slot) => {
                        assert_eq!(
                            slot.get(),
                            result,
                            "one assignment attempt published conflicting terminal payloads"
                        );
                        false
                    }
                }
            });
            let first_product_publication = first_publication
                && recorded.as_ref().is_some_and(|result| {
                    result.status == ResultStatus::Success && !result.repos.is_empty()
                });
            if first_product_publication {
                // Durable replay must not masquerade as a second workspace push.
                hooks.reach(PausePoint::WorkerPushCompleted).await;
            }
            if recorded.is_some() {
                hooks.reach(PausePoint::ResultApplicationStarted).await;
            }
            let daemon = self.router.current();
            let reply = daemon
                .deliver_protocol_message_with_auth(message, auth)
                .await;
            if let Ok(Some(WorkerProtocolMessage::Release(release))) = &reply {
                published_releases
                    .lock()
                    .expect("published release lock")
                    .push(release.clone());
            }
            if reports_jobs && matches!(&reply, Ok(None)) {
                hooks.reach(PausePoint::WorkerHeartbeatCompleted).await;
            }
            if terminal_activity
                && matches!(&reply, Ok(Some(WorkerProtocolMessage::ActivityAck(_))))
            {
                hooks
                    .reach(PausePoint::WorkerTerminalTraceAcknowledgement)
                    .await;
            }
            if matches!(&reply, Ok(Some(WorkerProtocolMessage::Assign(_)))) {
                // The daemon only emits Assign after the durable claim CAS.
                hooks.reach(PausePoint::AssignmentClaimCommitted).await;
            }
            if let Some(result) = recorded {
                if reply.is_ok() {
                    hooks.reach(PausePoint::ResultApplicationCompleted).await;
                }
                let _ = result_tx.send(result);
            }
            reply
        }
    }
}
