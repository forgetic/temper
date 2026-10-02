//! The network between the worker and the engine: one channel at a time,
//! which the worker dials, with a latency each way, that drops at drawn
//! moments, after which the engine is out of reach for a drawn while, during
//! which every dial fails. What is in flight on a channel that drops is lost,
//! both ways; each end hears of the loss, the engine once it has heard the
//! channel's hello, after what was sent before it.

use temper_fake_engine_model as engine;
use temper_lib::Duration;
use temper_worker_model::Event;
use temper_world::Span;

use super::{Delivery, Lane, WORKER, World};
use crate::translate;

/// How the network between the worker and the engine behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Network {
    /// How long a message takes on the channel, either way.
    pub hop: Span,
    /// How long a dial takes to open the channel, or to fail.
    pub dial: Span,
    /// The most channels that drop. Each drops with the chance `drop`, per
    /// mille, a drawn `life` after it opened; the engine is then out of reach
    /// for a drawn `outage`, during which every dial fails.
    pub drops: u32,
    pub drop: u32,
    pub life: Span,
    pub outage: Span,
}

/// The channel, as the network has it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Channel {
    /// None: the worker may dial.
    Idle,
    Dialling {
        epoch: u64,
    },
    /// Open, and whether the engine has heard its hello.
    Open {
        epoch: u64,
        hello: bool,
    },
    /// The worker's shell has stopped.
    Shut,
}

impl World {
    pub(super) fn dial(&mut self) {
        assert_eq!(self.channel, Channel::Idle, "one dial at a time, once the last has ended");
        self.epochs += 1;
        self.channel = Channel::Dialling { epoch: self.epochs };
        let at = self.now.saturating_add(self.settings.network.dial.draw(&mut self.rng));
        self.wire.send(at, Delivery::Dialled { epoch: self.epochs });
        self.stats.dials += 1;
    }

    /// The dial `epoch` ends: the channel opens, unless the engine is out of
    /// reach.
    pub(super) fn dialled(&mut self, epoch: u64) {
        match self.channel {
            Channel::Dialling { epoch: dialling } => assert_eq!(dialling, epoch, "a dial ends once"),
            Channel::Shut => return,
            Channel::Idle | Channel::Open { .. } => unreachable!("a dial ends once"),
        }
        if self.now < self.outage {
            self.channel = Channel::Idle;
            self.stats.failed_dials += 1;
            self.stage.push(Event::Lost);
            return;
        }
        self.channel = Channel::Open { epoch, hello: false };
        self.stats.connects += 1;
        self.stage.push(Event::Connected);
        let network = self.settings.network;
        if self.drops > 0 && self.rng.chance(network.drop) {
            self.drops -= 1;
            let at = self.now.saturating_add(network.life.draw(&mut self.rng));
            self.wire.send(at, Delivery::Drop { epoch });
        }
    }

    /// The channel `epoch` drops: what is in flight on it is lost, both ends
    /// hear of it, and the engine is out of reach for a while.
    pub(super) fn drop_channel(&mut self, epoch: u64) {
        let hello = match self.channel {
            Channel::Open { epoch: open, hello } if open == epoch => hello,
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => return,
        };
        self.channel = Channel::Idle;
        self.stats.drops += 1;
        self.outage = self.now.saturating_add(self.settings.network.outage.draw(&mut self.rng));
        let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.rng));
        self.wire.send(at, Delivery::Worker(Event::Lost));
        if hello {
            let at = self.lane(Lane::Up, Duration::ZERO);
            self.wire.send(at, Delivery::Engine(engine::Event::Lost { worker: WORKER }));
        }
    }

    /// Sends `event` up the channel, if one is open: lost otherwise.
    pub(super) fn send_up(&mut self, event: engine::Event) {
        match self.channel {
            Channel::Open { epoch, .. } => {
                let at = self.lane(Lane::Up, Duration::ZERO);
                self.wire.send(at, Delivery::Up { epoch, event });
            }
            Channel::Idle | Channel::Dialling { .. } | Channel::Shut => self.lost_up(&event),
        }
    }

    fn lost_up(&mut self, event: &engine::Event) {
        match event {
            engine::Event::Answered { .. } => self.stats.answers_lost += 1,
            engine::Event::Hello { .. }
            | engine::Event::Relay { .. }
            | engine::Event::Bounced { .. }
            | engine::Event::Fact { .. } => self.stats.lost_up += 1,
            engine::Event::Lost { .. } => unreachable!("the channel's loss goes off the channel"),
        }
    }

    pub(super) fn arrived_up(&mut self, epoch: u64, event: engine::Event) {
        match self.channel {
            Channel::Open { epoch: open, hello } if open == epoch => {
                let hello = match &event {
                    engine::Event::Hello { .. } => true,
                    engine::Event::Answered { attempt, .. } => {
                        self.stats.answers_taken += 1;
                        self.reached.insert(*attempt);
                        hello
                    }
                    engine::Event::Relay { .. }
                    | engine::Event::Bounced { .. }
                    | engine::Event::Fact { .. }
                    | engine::Event::Lost { .. } => hello,
                };
                self.channel = Channel::Open { epoch, hello };
                self.desk.push(event);
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => self.lost_up(&event),
        }
    }

    /// The worker's shell stops, once it is done: its channel closes.
    pub(super) fn stop(&mut self) {
        self.done = true;
        self.stats.done = true;
        if let Channel::Open { hello: true, .. } = self.channel {
            let at = self.lane(Lane::Up, Duration::ZERO);
            self.wire.send(at, Delivery::Engine(engine::Event::Lost { worker: WORKER }));
        }
        self.channel = Channel::Shut;
        self.stage.inbox.clear();
    }

    /// Sends the engine's `request` down the channel the engine has heard the
    /// hello of, if it is still open: lost otherwise.
    pub(super) fn send_down(&mut self, request: engine::Request) {
        let worker = match &request {
            engine::Request::Assign { worker, .. }
            | engine::Request::Inbound { worker, .. }
            | engine::Request::Cancel { worker, .. }
            | engine::Request::Relayed { worker, .. }
            | engine::Request::Acknowledge { worker, .. } => *worker,
        };
        assert_eq!(worker, WORKER, "the engine sends to the one worker there is");
        match self.channel {
            Channel::Open { epoch, hello: true } => {
                let event = translate::down(request, self.commit, &mut self.places);
                let at = self.lane(Lane::Down, Duration::ZERO);
                self.wire.send(at, Delivery::Down { epoch, event });
            }
            Channel::Open { hello: false, .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                self.stats.lost_down += 1;
            }
        }
    }
}
