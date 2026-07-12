// SPDX-License-Identifier: MPL-2.0

//! Reusable in-process worker→daemon transport.
//!
//! This crate is intentionally tiny composition glue between the engine and
//! worker layers: it wraps a co-resident [`temper_engine::Daemon`] as a
//! [`temper_worker::Transport`]. Keeping the glue in its own crate lets CLI
//! standalone mode, deterministic simulation, and hermetic real-stack tests use
//! the same carrier without making the normal `temper-worker` crate depend on
//! `temper-engine`.

use std::future::Future;

use skein::cx::Cx;
use temper_engine::Daemon;
use temper_protocol_worker::{WorkerAuth, WorkerProtocolMessage};
use temper_worker::{ForgeContextClient, Transport};

/// Co-resident context client. It uses the same authenticated protocol DTOs and
/// daemon route as [`temper_worker::HttpForgeContextClient`] without a socket.
pub type InProcessForgeContextClient = ForgeContextClient<InProcessTransport>;

/// In-process worker→daemon transport.
///
/// Wraps a clone of the same [`Daemon`] the daemon loop owns; [`Transport::send`]
/// calls [`Daemon::deliver_protocol_message`], which hands the message to the
/// daemon machine and awaits its reply — the identical path the HTTP listener
/// drives, minus TCP and the HTTP byte round-trip.
#[derive(Clone)]
pub struct InProcessTransport {
    daemon: Daemon,
}

impl InProcessTransport {
    /// Bind the transport to a co-resident daemon handle.
    pub fn new(daemon: Daemon) -> Self {
        Self { daemon }
    }
}

impl Transport for InProcessTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let daemon = self.daemon.clone();
        async move {
            daemon
                .deliver_protocol_message_with_auth(message, auth)
                .await
        }
    }
}
