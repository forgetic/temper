//! The network between the worker and the engine: one channel at a time,
//! which the worker dials, with a latency each way, that drops at drawn
//! moments, after which the engine is out of reach for a drawn while, during
//! which every dial fails. What is in flight on a channel that drops is lost,
//! both ways; each end hears of the loss, the engine once its protocol layer
//! has handed it the channel's hello, after what was sent before it.
//!
//! Each end's protocol layer translates what crosses ([`crate::protocol`]),
//! and takes liberties a channel's frames show. Now and then the channel
//! stalls for a while, everything behind the frame waiting with it; a frame
//! is never reordered, and one that is lost goes with its channel. And now
//! and then a protocol layer sends a frame again, of those its receiver must
//! take again harmlessly, at a moment the frame allows ([`Moment`]): right
//! behind it, or behind the next frame; an engine's cancel, an answer to a
//! relayed call, or an inbound event once the attempt it is for has
//! answered, when the worker no longer hosts it; and an engine's cancel
//! again on a new channel, behind the hello that lists its attempt. The
//! copies, their
//! moments and their latencies are drawn apart from everything else the
//! world draws, so that a world with copies and one without draw the same
//! for the rest.

use skein_lib::{Duration, Time, Token};
use temper_engine_domain as engine;
use temper_worker_domain::{Event, host};
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
    /// harmlessly is sent again; and that the channel stalls before a frame,
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

/// When a copy of a frame arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Moment {
    /// Right behind the frame.
    Now,
    /// Behind the next frame sent the same way on the channel.
    Behind,
    /// Once the attempt it is for has answered.
    Answered(Names),
}

/// A copy of a frame sent on the channel `epoch` the `lane` way, held back
/// until its moment.
#[derive(Debug)]
pub(super) struct Held {
    epoch: u64,
    lane: Lane,
    moment: Moment,
    delivery: Delivery,
}

/// The most copies held back at once. A world past it fails with its seed.
const HELD: usize = 1_000;

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
        let drops = self.rng.chance(self.settings.network.drop);
        self.hosting.observe(self.now, Seen::Opened { epoch, drops }, &mut Vec::new());
    }

    /// The channel `epoch` drops, if it is still the one open: what is in
    /// flight on it is lost, both ends hear of it, and the engine is out of
    /// reach for a while.
    pub(super) fn drop_channel(&mut self, epoch: u64) {
        let hello = match self.channel {
            Channel::Open { epoch: open, hello } if open == epoch => hello,
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => return,
        };
        self.channel = Channel::Idle;
        self.stats.drops += 1;
        self.end("dropped");
        self.lose_held(epoch);
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
        let event = engine::Event::Lost { channel: token(epoch) };
        self.send(at, Delivery::Engine { life: self.engine_life, event });
        self.hosting.observe(self.now, Seen::Lost, &mut Vec::new());
    }

    /// Sends up the channel, if one is open, what `make` makes of the
    /// engine's name for it: lost otherwise.
    pub(super) fn send_up(&mut self, make: impl FnOnce(Token) -> engine::Event) {
        match self.channel {
            Channel::Open { epoch, .. } => {
                let event = make(token(epoch));
                let moments: &[Moment] = match &event {
                    engine::Event::Answer { answer, .. } if !refusal(answer) => &[Moment::Now, Moment::Behind],
                    engine::Event::Answer { .. }
                    | engine::Event::Told { .. }
                    | engine::Event::Hello { .. }
                    | engine::Event::Relay { .. }
                    | engine::Event::Bounced { .. }
                    | engine::Event::Answered { .. }
                    | engine::Event::Hint { .. }
                    | engine::Event::Lost { .. }
                    | engine::Event::Ask { .. }
                    | engine::Event::Unwatch { .. }
                    | engine::Event::Delivered { .. }
                    | engine::Event::Stored { .. } => &[],
                };
                let at = self.frame(Lane::Up);
                let copy = self.moment(moments).map(|moment| (moment, copy(&event)));
                self.send(at, Delivery::Up { epoch, event, copy: false });
                self.behind(Lane::Up, epoch, at);
                if let Some((moment, event)) = copy {
                    self.send_copy(moment, epoch, Lane::Up, at, Delivery::Up { epoch, event, copy: true });
                }
            }
            Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                let event = make(token(0));
                self.lost_up(&event, false);
            }
        }
    }

    /// When a copy of a frame comes, of the `moments` its frame allows, if
    /// one does.
    fn moment(&mut self, moments: &[Moment]) -> Option<Moment> {
        if moments.is_empty() || !self.copies.chance(self.settings.network.duplicates) {
            return None;
        }
        let at = self.copies.below(u64::try_from(moments.len()).expect("few"));
        Some(moments[usize::try_from(at).expect("few")])
    }

    /// Sends the copy `delivery` of a frame sent on the channel `epoch` the
    /// `lane` way, to arrive at `at`: then, or held back until its moment.
    fn send_copy(&mut self, moment: Moment, epoch: u64, lane: Lane, at: Time, delivery: Delivery) {
        self.stats.duplicated += 1;
        match moment {
            Moment::Now => {
                self.send(at, delivery);
            }
            Moment::Behind | Moment::Answered(_) => {
                assert!(self.held.len() < HELD, "seed {}: more copies held than the world holds", self.settings.seed);
                self.held.push(Held { epoch, lane, moment, delivery });
            }
        }
    }

    /// A frame sent on the channel `epoch` the `lane` way arrives at `at`:
    /// the copies held behind the next such frame follow it.
    fn behind(&mut self, lane: Lane, epoch: u64, at: Time) {
        let (due, held): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|held| held.moment == Moment::Behind && held.lane == lane && held.epoch == epoch);
        self.held = held;
        for held in due {
            self.send(at, held.delivery);
        }
    }

    /// The worker answered the attempt `names`: the copies held for it go
    /// down the channel they were sent on, if it is still open, a hop later.
    pub(super) fn answered_copies(&mut self, names: Names) {
        let (due, held): (Vec<Held>, Vec<Held>) =
            std::mem::take(&mut self.held).into_iter().partition(|held| held.moment == Moment::Answered(names));
        self.held = held;
        for held in due {
            match self.channel {
                Channel::Open { epoch, .. } if epoch == held.epoch => {
                    let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.copies));
                    self.send(at, held.delivery);
                }
                Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                    self.stats.copies_lost += 1;
                }
            }
        }
    }

    /// The engine's protocol layer has the hello of the channel `epoch`: now
    /// and then, it sends again the cancels the engine sent before for the
    /// attempts it lists.
    fn recancel(&mut self, epoch: u64, listed: &[Names]) {
        for names in listed {
            if !self.engine_cancels.contains(names) || !self.copies.chance(self.settings.network.duplicates) {
                continue;
            }
            self.stats.duplicated += 1;
            self.stats.recancels += 1;
            let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.copies));
            let event = Event::Cancel { run: names.0, attempt: names.1 };
            self.send(at, Delivery::Down { epoch, event, copy: true });
        }
    }

    /// The channel `epoch` closed: the copies held on it go with it.
    pub(super) fn lose_held(&mut self, epoch: u64) {
        self.acknowledged.retain(|(on, _)| *on != epoch);
        let (lost, held): (Vec<Held>, Vec<Held>) =
            std::mem::take(&mut self.held).into_iter().partition(|held| held.epoch == epoch);
        self.held = held;
        self.stats.copies_lost += u32::try_from(lost.len()).expect("few");
    }

    /// The world settled: the copies still held are never sent.
    pub(super) fn forget_held(&mut self) {
        self.stats.copies_lost += u32::try_from(self.held.len()).expect("few");
        self.held.clear();
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

    /// What went up the channel is lost: the frame, or a copy of it.
    fn lost_up(&mut self, event: &engine::Event, copy: bool) {
        if copy {
            self.stats.copies_lost += 1;
            return;
        }
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

    /// What went up the channel `epoch` reaches the engine's protocol layer,
    /// if the channel is still open: the frame, or a copy of it.
    pub(super) fn arrived_up(&mut self, epoch: u64, event: engine::Event, copy: bool) {
        match self.channel {
            Channel::Open { epoch: open, hello } if open == epoch => {
                let mut listed = None;
                let hello = match &event {
                    engine::Event::Hello { hello, .. } => {
                        let hosting = hello.hosting.iter().map(|hosted| protocol::names(hosted.item, hosted.attempt));
                        listed = Some(hosting.collect::<Vec<Names>>());
                        let answered = hello.hosting.iter().filter_map(|hosted| match hosted.phase {
                            engine::fleet::Phase::Answered => Some(protocol::names(hosted.item, hosted.attempt)),
                            engine::fleet::Phase::Preparing
                            | engine::fleet::Phase::Starting
                            | engine::fleet::Phase::Active
                            | engine::fleet::Phase::Waiting
                            | engine::fleet::Phase::Ending => None,
                        });
                        let answered = answered.collect();
                        self.hosting.observe(self.now, Seen::Hello { answered }, &mut Vec::new());
                        true
                    }
                    engine::Event::Answer { item, attempt, answer, .. } => {
                        let names = protocol::names(*item, *attempt);
                        if copy {
                            self.stats.answers_copied += 1;
                        } else {
                            self.stats.answers_taken += 1;
                        }
                        self.reached.insert(names);
                        let refused = refusal(answer);
                        self.hosting.observe(self.now, Seen::Answered { names, refused }, &mut Vec::new());
                        self.stories.observe(
                            self.now,
                            temper_engine_domain_world::referee::Seen::Answered { item: *item, attempt: *attempt },
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
                if let Some(listed) = listed {
                    self.recancel(epoch, &listed);
                }
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                self.lost_up(&event, copy);
            }
        }
    }

    /// The worker's shell stops, once it is done: its channel closes, and a
    /// new worker starts a while later.
    pub(super) fn stop(&mut self) {
        self.done = true;
        self.stats.done = true;
        if let Channel::Open { epoch, hello } = self.channel {
            if hello {
                self.lose(epoch);
            }
            self.lose_held(epoch);
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
    /// if it is still the one open: lost otherwise.
    pub(super) fn send_down(&mut self, channel: Token, event: Event) {
        match self.channel {
            Channel::Open { epoch, hello: true } if token(epoch) == channel => {
                let moments: &[Moment] = match &event {
                    Event::Acknowledged { .. } => &[Moment::Now, Moment::Behind],
                    Event::Assign { .. } => &[Moment::Now],
                    Event::Cancel { run, attempt } => {
                        let names = (*run, *attempt);
                        &[Moment::Now, Moment::Behind, Moment::Answered(names)]
                    }
                    Event::Relayed { run, attempt, .. } => {
                        &[Moment::Now, Moment::Behind, Moment::Answered((*run, *attempt))]
                    }
                    Event::Inbound { run, attempt, .. } => &[Moment::Answered((*run, *attempt))],
                    Event::RelayCancelled { .. }
                    | Event::Connected
                    | Event::Lost
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
                    | Event::Done { .. } => unreachable!("only the engine's words go down the channel"),
                };
                // The engine acknowledges an answer again on the same channel
                // only for a copy of it: what a copy causes is timed apart.
                let again = match &event {
                    Event::Acknowledged { run, attempt } => !self.acknowledged.insert((epoch, (*run, *attempt))),
                    Event::Assign { .. }
                    | Event::Cancel { .. }
                    | Event::Relayed { .. }
                    | Event::Inbound { .. }
                    | Event::RelayCancelled { .. }
                    | Event::Connected
                    | Event::Lost
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
                    | Event::Done { .. } => false,
                };
                if again {
                    let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.copies));
                    self.send(at, Delivery::Down { epoch, event, copy: false });
                    return;
                }
                let moment = self.moment(moments);
                let at = self.frame(Lane::Down);
                let copy = moment.map(|moment| (moment, copy_down(&event)));
                self.send(at, Delivery::Down { epoch, event, copy: false });
                self.behind(Lane::Down, epoch, at);
                if let Some((moment, event)) = copy {
                    self.send_copy(moment, epoch, Lane::Down, at, Delivery::Down { epoch, event, copy: true });
                }
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                self.stats.lost_down += 1;
            }
        }
    }

    /// What went down the channel `epoch` reaches the worker's protocol
    /// layer, if the channel is still open: the frame, or a copy of it,
    /// counted if it comes once the attempt it is for has answered.
    pub(super) fn arrived_down(&mut self, epoch: u64, event: Event, copy: bool) {
        match self.channel {
            Channel::Open { epoch: open, .. } if open == epoch => {
                if copy {
                    self.late(&event);
                }
                self.push_worker_event(event);
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut if copy => {
                self.stats.copies_lost += 1;
            }
            Channel::Open { .. } | Channel::Idle | Channel::Dialling { .. } | Channel::Shut => {
                self.stats.lost_down += 1;
            }
        }
    }

    /// Counts a copy that reaches the worker once the attempt it is for has
    /// answered, or was given again: one it takes again, harmlessly.
    fn late(&mut self, event: &Event) {
        let (kind, names) = match event {
            Event::Cancel { run, attempt } => ("cancel", (*run, *attempt)),
            Event::Relayed { run, attempt, .. } => ("relayed", (*run, *attempt)),
            Event::Inbound { run, attempt, .. } => ("inbound", (*run, *attempt)),
            Event::Acknowledged { .. } | Event::Assign { .. } => return,
            Event::RelayCancelled { .. }
            | Event::Connected
            | Event::Lost
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
            | Event::Done { .. } => unreachable!("only the engine's words go down the channel"),
        };
        if self.attempts.get(&names).is_none_or(|record| record.answer.is_some()) {
            *self.stats.late.entry(kind).or_default() += 1;
        }
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
        | engine::Event::Stored { .. } => unreachable!("only what is taken again is sent again"),
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
        Event::Assign { assignment } => Event::Assign { assignment: copy_assignment(assignment) },
        Event::Inbound { run, attempt, event } => Event::Inbound { run: *run, attempt: *attempt, event: event.clone() },
        Event::RelayCancelled { .. }
        | Event::Connected
        | Event::Lost
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
        | Event::Done { .. } => unreachable!("only what is taken again is sent again"),
    }
}

/// A copy of an assignment, as the engine's protocol layer sends it again.
fn copy_assignment(assignment: &host::Assignment) -> host::Assignment {
    let repositories = assignment.workspace.repositories.iter().map(|repository| host::Repository {
        name: repository.name.clone(),
        remote: repository.remote.clone(),
        start: match &repository.start {
            host::Start::Base { branch } => host::Start::Base { branch: branch.clone() },
            host::Start::Branch { branch } => host::Start::Branch { branch: branch.clone() },
            host::Start::Commit { commit } => host::Start::Commit { commit: *commit },
            host::Start::Saved { branch } => host::Start::Saved { branch: branch.clone() },
        },
        access: match &repository.access {
            host::Access::Writable { push } => host::Access::Writable { push: push.clone() },
            host::Access::ReadOnly => host::Access::ReadOnly,
        },
        identity: repository.identity.clone(),
    });
    host::Assignment {
        run: assignment.run,
        attempt: assignment.attempt,
        workspace: host::Workspace { key: assignment.workspace.key.clone(), repositories: repositories.collect() },
        save: assignment.save.clone(),
        charter: assignment.charter.clone(),
        snapshot: assignment.snapshot.clone(),
    }
}
