// SPDX-License-Identifier: MPL-2.0

//! The standalone worker→daemon transport: deliver worker-protocol messages
//! straight to a co-resident [`Daemon`] over an in-memory call, with no TCP and
//! no HTTP byte round-trip.

use std::future::Future;

use skein::cx::Cx;
use temper_engine::Daemon;
use temper_protocol_worker::WorkerProtocolMessage;
use temper_worker::Transport;

/// In-process worker→daemon transport. Wraps a clone of the same [`Daemon`] the
/// daemon loop owns; `send` calls [`Daemon::deliver_protocol_message`], which
/// hands the message to the daemon machine and awaits its reply — the identical
/// path the HTTP listener drives, minus the socket.
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
