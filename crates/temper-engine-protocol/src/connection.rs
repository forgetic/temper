//! One authenticated link. The listener owns names, timers and io bindings.
use skein_lib::{Queue, Time, stream};
use temper_channel::{Sizes, machine};

use crate::Limits;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    /// Accepted only from a loopback peer, on a loopback listener.
    Loopback,
    /// The external TLS owner supplies plaintext only after authentication.
    Secured,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Securing,
    Opening,
    /// Authenticated replacement waiting for the old socket's actual Closed.
    Pending,
    Hello,
    Open,
    Closing,
    Closed,
}

#[derive(Debug)]
pub(crate) struct Connection {
    pub(crate) socket: skein_lib::Token,
    pub(crate) transport: Transport,
    pub(crate) phase: Phase,
    pub(crate) machine: machine::Machine,
    pub(crate) upper: Queue<machine::Event>,
    pub(crate) lower: Queue<stream::Down>,
    pub(crate) worker: Option<u32>,
    pub(crate) announced: bool,
    pub(crate) until: Time,
    pub(crate) heard: Time,
    pub(crate) sent: Time,
    pub(crate) pinged: Time,
    pub(crate) progress: Time,
    pub(crate) debt: bool,
    pub(crate) aborted: bool,
}

impl Connection {
    pub(crate) fn new(
        socket: skein_lib::Token,
        transport: Transport,
        now: Time,
        limits: &Limits,
        sizes: &Sizes,
    ) -> Option<Connection> {
        Some(Connection {
            socket,
            transport,
            phase: match transport {
                Transport::Loopback => Phase::Opening,
                Transport::Secured => Phase::Securing,
            },
            machine: machine::Machine::new(machine::Endpoint::Engine, &limits.channel, sizes)?,
            upper: Queue::with_capacity(machine::MAX_UP),
            lower: Queue::with_capacity(machine::MAX_DOWN),
            worker: None,
            announced: false,
            until: now.saturating_add(limits.channel.handshake),
            heard: now,
            sent: now,
            pinged: now,
            progress: now,
            debt: false,
            aborted: false,
        })
    }

    pub(crate) fn request(&mut self, limits: &Limits, sizes: &Sizes, request: machine::Request) {
        machine::down(&mut self.machine, &limits.channel, sizes, request, &mut self.upper, &mut self.lower);
    }

    pub(crate) fn input(&mut self, limits: &Limits, sizes: &Sizes, now: Time, event: stream::Up) {
        if event == stream::Up::Room {
            self.progress = now;
        }
        let received = self.machine.received_frames();
        machine::up(&mut self.machine, &limits.channel, sizes, event, &mut self.upper, &mut self.lower);
        if self.machine.received_frames() != received {
            self.heard = now;
        }
    }

    pub(crate) fn poll(&mut self, limits: &Limits, sizes: &Sizes) {
        machine::poll(&mut self.machine, &limits.channel, sizes, &mut self.upper, &mut self.lower);
    }

    pub(crate) fn deadline(&self, limits: &Limits) -> Option<Time> {
        match self.phase {
            Phase::Securing | Phase::Opening | Phase::Pending => Some(self.until),
            Phase::Hello => Some(self.until.min(self.sent.max(self.pinged).saturating_add(limits.channel.ping))),
            Phase::Open => {
                let mut next = self
                    .heard
                    .saturating_add(limits.channel.silence)
                    .min(self.sent.max(self.pinged).saturating_add(limits.channel.ping));
                if self.debt {
                    next = next.min(self.progress.saturating_add(limits.channel.stall));
                }
                Some(next)
            }
            Phase::Closing if !self.aborted => Some(self.until),
            Phase::Closing | Phase::Closed => None,
        }
    }

    pub(crate) fn stalled(&self, now: Time, limits: &Limits) -> bool {
        self.debt && self.progress.saturating_add(limits.channel.stall) <= now
    }
}
