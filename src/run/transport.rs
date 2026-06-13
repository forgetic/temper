// SPDX-License-Identifier: MPL-2.0

//! The unified-mode worker→daemon transport: deliver worker-protocol messages
//! straight to a co-resident [`Daemon`] over an in-memory call, with no TCP and
//! no HTTP byte round-trip.

use std::future::Future;

use skein::cx::Cx;
use temper_daemon::Daemon;
use temper_worker_orchestrator::Transport;
use temper_worker_protocol::WorkerProtocolMessage;

/// In-process worker→daemon transport. Wraps a clone of the same [`Daemon`] the
/// daemon loop owns; `send` calls [`Daemon::deliver_protocol_message`], which
/// hands the message to the daemon machine as a `DaemonCompletion::Http` and
/// awaits its reply — the identical path the HTTP listener drives, minus the
/// socket. Long-poll semantics carry over for free.
#[derive(Clone)]
pub struct InProcessTransport {
    daemon: Daemon,
}

impl InProcessTransport {
    pub fn new(daemon: Daemon) -> Self {
        Self { daemon }
    }
}

impl Transport for InProcessTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let daemon = self.daemon.clone();
        async move { daemon.deliver_protocol_message(message).await }
    }
}
