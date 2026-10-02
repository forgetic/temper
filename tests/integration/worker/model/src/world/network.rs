//! The network between the worker and the engine: one channel at a time,
//! which the worker dials, with a latency each way, that drops at drawn
//! moments, after which the engine is out of reach for a drawn while, during
//! which every dial fails. What is in flight on a channel that drops is lost,
//! both ways; each end hears of the loss, the engine once its protocol layer
//! has handed it the channel's hello, after what was sent before it.
//!
//! Each end's protocol layer translates what crosses ([`crate::protocol`]),
//! and takes liberties a channel's frames show: now and then it sends a frame
//! twice, of those its receiver must take again harmlessly (an
//! acknowledgement, a cancel, an answer to a relayed call, a run's answer,
//! a fact); and now and then the channel stalls for a while, everything
//! behind the frame waiting with it. A frame is never reordered, and one
//! that is lost goes with its channel.

use temper_engine_model as engine;
use temper_lib::{Duration, Time, Token};
use temper_worker_model::Event;
use temper_world::Span;

use super::{Delivery, Lane, World};
use crate::protocol::{self, Names};
use crate::referee::Seen;

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
    /// The chance, per mille, that a frame its receiver takes again
    /// harmlessly is sent twice; and that the channel stalls before a frame,
    /// for a drawn `stall`.
    pub duplicates: u32,
    pub stalls: u32,
    pub stall: Span,
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

/// The engine's name for the channel `epoch`.
fn token(epoch: u64) -> Token {
    Token::new(epoch)
}

impl World {
    pub(super) fn dial(&mut self) {
        assert_eq!(self.channel, Channel::Idle, "one dial at a time, once the last has ended");
        self.epochs += 1;
        self.channel = Channel::Dialling { epoch: self.epochs };
        let at = self.now.saturating_add(self.settings.network.dial.draw(&mut self.rng));
        self.send(at, Delivery::Dialled { epoch: self.epochs });
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
            self.send(at, Delivery::Drop { epoch });
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
        self.end("dropped");
        self.outage = self.now.saturating_add(self.settings.network.outage.draw(&mut self.rng));
        let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.rng));
        self.send(at, Delivery::Worker(self.lives, Event::Lost));
        if hello {
            self.lose(epoch);
        }
    }

    /// The engine's protocol layer hears the channel `epoch` lost, after what
    /// came up it before.
    fn lose(&mut self, epoch: u64) {
        let at = self.lane(Lane::Up, Duration::ZERO);
        self.send(at, Delivery::Engine(engine::Event::Lost { channel: token(epoch) }));
        self.hosting.observe(self.now, Seen::Lost, &mut Vec::new());
    }

    /// Sends up the channel, if one is open, what `make` makes of the
    /// engine's name for it: lost otherwise.
    pub(super) fn send_up(&mut self, make: impl FnOnce(Token) -> engine::Event) {
        match self.channel {
            Channel::Open { epoch, .. } => {
                let event = make(token(epoch));
                let twice = match &event {
                    engine::Event::Answer { answer, .. } => !refusal(answer),
                    engine::Event::Told { .. } => true,
                    engine::Event::Hello { .. }
                    | engine::Event::Relay { .. }
                    | engine::Event::Bounced { .. }
                    | engine::Event::Answered { .. }
                    | engine::Event::Hint { .. }
                    | engine::Event::Lost { .. }
                    | engine::Event::Ask { .. }
                    | engine::Event::Unwatch { .. }
                    | engine::Event::Delivered { .. }
                    | engine::Event::Stored { .. } => false,
                };
                let at = self.frame(Lane::Up);
                if twice && self.rng.chance(self.settings.network.duplicates) {
                    self.stats.duplicated += 1;
                    let copy = copy(&event);
                    self.send(at, Delivery::Up { epoch, event: copy });
                }
                self.send(at, Delivery::Up { epoch, event });
            }
            Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                let event = make(token(0));
                self.lost_up(&event);
            }
        }
    }

    /// When the next frame on `lane` arrives: a hop later, after what was sent
    /// before it, and after a stall, now and then.
    fn frame(&mut self, lane: Lane) -> Time {
        let network = self.settings.network;
        let stall = if self.rng.chance(network.stalls) {
            self.stats.stalled += 1;
            network.stall.draw(&mut self.rng)
        } else {
            Duration::ZERO
        };
        self.lane(lane, stall)
    }

    fn lost_up(&mut self, event: &engine::Event) {
        match event {
            engine::Event::Answer { .. } => self.stats.answers_lost += 1,
            engine::Event::Hello { .. }
            | engine::Event::Relay { .. }
            | engine::Event::Bounced { .. }
            | engine::Event::Told { .. } => self.stats.lost_up += 1,
            engine::Event::Answered { .. }
            | engine::Event::Hint { .. }
            | engine::Event::Lost { .. }
            | engine::Event::Ask { .. }
            | engine::Event::Unwatch { .. }
            | engine::Event::Delivered { .. }
            | engine::Event::Stored { .. } => unreachable!("only the worker's words go up the channel"),
        }
    }

    pub(super) fn arrived_up(&mut self, epoch: u64, event: engine::Event) {
        match self.channel {
            Channel::Open { epoch: open, hello } if open == epoch => {
                let hello = match &event {
                    engine::Event::Hello { .. } => true,
                    engine::Event::Answer { item, attempt, answer, .. } => {
                        let names = protocol::names(*item, *attempt);
                        self.stats.answers_taken += 1;
                        self.reached.insert(names);
                        let refused = refusal(answer);
                        self.hosting.observe(self.now, Seen::Answered { names, refused }, &mut Vec::new());
                        self.stories.observe(
                            self.now,
                            temper_engine_model_tests::referee::Seen::Answered { item: *item, attempt: *attempt },
                            &mut Vec::new(),
                        );
                        hello
                    }
                    engine::Event::Relay { item, attempt, call, .. } => {
                        let names = protocol::names(*item, *attempt);
                        self.hosting.observe(self.now, Seen::Relay { names, call: *call }, &mut Vec::new());
                        hello
                    }
                    engine::Event::Bounced { .. } | engine::Event::Told { .. } => hello,
                    engine::Event::Answered { .. }
                    | engine::Event::Hint { .. }
                    | engine::Event::Lost { .. }
                    | engine::Event::Ask { .. }
                    | engine::Event::Unwatch { .. }
                    | engine::Event::Delivered { .. }
                    | engine::Event::Stored { .. } => unreachable!("only the worker's words go up the channel"),
                };
                self.hosting.assert_holding(self.settings.seed);
                self.stories.assert_holding(self.settings.seed);
                self.channel = Channel::Open { epoch, hello };
                self.desk.push(event);
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => self.lost_up(&event),
        }
    }

    /// The worker's shell stops, once it is done: its channel closes, and a
    /// new worker starts a while later.
    pub(super) fn stop(&mut self) {
        self.done = true;
        self.stats.done = true;
        if let Channel::Open { epoch, hello: true } = self.channel {
            self.lose(epoch);
        }
        self.channel = Channel::Shut;
        self.stage.inbox.clear();
        let at = self.now.saturating_add(self.settings.comeback.draw(&mut self.rng));
        self.send(at, Delivery::Comeback);
    }

    /// Closes the channel `channel`, which the engine refuses: the worker
    /// hears it lost, and dials again.
    pub(super) fn refuse(&mut self, channel: Token) {
        let Channel::Open { epoch, .. } = self.channel else { return };
        if token(epoch) != channel {
            return;
        }
        self.end("refused");
        self.channel = Channel::Idle;
        let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.rng));
        self.send(at, Delivery::Worker(self.lives, Event::Lost));
    }

    /// Sends what the engine asks of the worker down the channel `channel`,
    /// if it is still the one open: lost otherwise. `twice` says whether the
    /// worker takes it again harmlessly.
    pub(super) fn send_down(&mut self, channel: Token, event: Event, twice: bool) {
        match self.channel {
            Channel::Open { epoch, hello: true } if token(epoch) == channel => {
                let at = self.frame(Lane::Down);
                if twice && self.rng.chance(self.settings.network.duplicates) {
                    self.stats.duplicated += 1;
                    self.send(at, Delivery::Down { epoch, event: copy_down(&event) });
                }
                self.send(at, Delivery::Down { epoch, event });
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                self.stats.lost_down += 1;
            }
        }
    }

    pub(super) fn arrived_down(&mut self, epoch: u64, event: Event) {
        match self.channel {
            Channel::Open { epoch: open, .. } if open == epoch => self.stage.push(event),
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                self.stats.lost_down += 1;
            }
        }
    }

    /// The worker's names for the item's attempt, as the protocol layer
    /// packs them.
    pub(super) fn names(item: engine::Item, attempt: u64) -> Names {
        protocol::names(item, attempt)
    }
}

/// Whether `answer` refuses its assignment: one the engine does not take
/// again, but places anew.
fn refusal(answer: &engine::Answer) -> bool {
    match answer {
        engine::Answer::Busy | engine::Answer::Invalid => true,
        engine::Answer::Ended { .. } | engine::Answer::Parked { .. } | engine::Answer::Failed { .. } => false,
    }
}

/// A copy of a frame the engine takes again harmlessly.
fn copy(event: &engine::Event) -> engine::Event {
    match event {
        engine::Event::Answer { channel, item, attempt, answer } => {
            engine::Event::Answer { channel: *channel, item: *item, attempt: *attempt, answer: answer.clone() }
        }
        engine::Event::Told { item, attempt, kind, content } => {
            engine::Event::Told { item: *item, attempt: *attempt, kind: *kind, content: content.clone() }
        }
        engine::Event::Hello { .. }
        | engine::Event::Relay { .. }
        | engine::Event::Bounced { .. }
        | engine::Event::Answered { .. }
        | engine::Event::Hint { .. }
        | engine::Event::Lost { .. }
        | engine::Event::Ask { .. }
        | engine::Event::Unwatch { .. }
        | engine::Event::Delivered { .. }
        | engine::Event::Stored { .. } => unreachable!("only what is taken again is sent twice"),
    }
}

/// A copy of a frame the worker takes again harmlessly.
fn copy_down(event: &Event) -> Event {
    match event {
        Event::Acknowledged { run, attempt } => Event::Acknowledged { run: *run, attempt: *attempt },
        Event::Cancel { run, attempt } => Event::Cancel { run: *run, attempt: *attempt },
        Event::Relayed { run, attempt, call, answer } => {
            Event::Relayed { run: *run, attempt: *attempt, call: *call, answer: answer.clone() }
        }
        Event::Connected
        | Event::Lost
        | Event::Assign { .. }
        | Event::Inbound { .. }
        | Event::Shutdown
        | Event::Spawned { .. }
        | Event::Unspawned { .. }
        | Event::Sent { .. }
        | Event::Unsent { .. }
        | Event::Received { .. }
        | Event::Malformed { .. }
        | Event::Hangup { .. }
        | Event::Signalled { .. }
        | Event::Exited { .. }
        | Event::Reaped { .. }
        | Event::Done { .. } => unreachable!("only what is taken again is sent twice"),
    }
}
