//! The engine's neighbours: the forge, through the engine's protocol layer
//! as the engine's world plays it (`temper_engine_domain_tests::translate`),
//! each call with a deadline; the store; people, who hand issues in, review,
//! and stop runs; and the forge's observations, which the mirror, the
//! referees and people see.

use temper_engine_domain::forge::api as engine_api;
use temper_engine_domain::{self as engine, Ask, Event, Item, Reply, Request};
use temper_engine_domain_tests::deployment::{self, PEOPLE};
use temper_engine_domain_tests::referee as engine_referee;
use temper_engine_domain_tests::translate;
use temper_forge_domain::{self as forge, Observation};
use temper_lib::{ReplyTo, Token};

use super::{Delivery, Out, World};
use crate::forge::{self as shared, Direct, OTHER};
use crate::referee::Seen;

impl World {
    /// The forge, met directly now, as `user`, reaching it if `reachable`.
    pub(super) fn direct(&mut self, user: u64, reachable: bool) -> Direct<'_> {
        Direct {
            domain: &mut self.forge,
            env: shared::env(&self.settings.forge, self.now),
            user,
            stray: &mut self.stray,
            calls: &mut self.direct,
            reachable,
        }
    }

    /// Routes what the forge emitted for others during direct calls.
    pub(super) fn route_stray(&mut self) {
        for request in std::mem::take(&mut self.stray) {
            self.forge_request(request);
        }
    }

    /// A person hands in the issue `hands[at]`.
    pub(super) fn hand_in(&mut self, at: usize) {
        let hand = self.settings.hands[at].clone();
        let person = PEOPLE[at % PEOPLE.len()];
        let number = self.direct(person, true).hand_in(&hand, person);
        self.route_stray();
        let item = Item { repository: hand.repository, number };
        self.items.insert(item, at);
        self.stats.handed += 1;
        self.observe(Seen::Handed { item });
        self.log(&format!("person {person} hands in {item:?} as {:?} {:?}", hand.job, hand.work));
        self.observe_forge();
        if !self.looking {
            self.looking = true;
            let gap = self.draw(self.settings.people);
            self.send_at(self.now.saturating_add(gap), Delivery::Look);
        }
    }

    /// The reviewer looks at the forge as observed, and reviews what is due;
    /// and looks again later, while any issue handed in is open.
    pub(super) fn review(&mut self) {
        self.looking = false;
        let mut reviews = Vec::new();
        self.reviewer.look(&self.mirror, &mut reviews);
        for review in reviews {
            let name = self.wire.name();
            self.theirs.open(name, (review.repository, review.number, review.head));
            self.log(&format!("the reviewer approves #{} at {}", review.number, review.head));
            let event = forge::Event::Call {
                reply_to: ReplyTo::new(Token::new(name)),
                user: deployment::REVIEWER,
                repository: deployment::REPOSITORIES[review.repository].into(),
                op: review.op,
            };
            forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
            self.drain_forge();
        }
        let open = self.items.keys().any(|item| {
            let repository = deployment::name(item.repository);
            self.mirror.issue(repository, item.number).is_none_or(|issue| issue.open)
        });
        if open || self.items.len() < self.settings.hands.len() {
            self.looking = true;
            let gap = self.draw(self.settings.people);
            self.send_at(self.now.saturating_add(gap), Delivery::Look);
        }
    }

    /// A person asks the engine to stop the item's run.
    pub(super) fn stop(&mut self, item: Item) {
        let name = self.wire.name();
        self.asks.open(name, item);
        self.stats.stops += 1;
        self.log(&format!("a person stops {item:?}"));
        let ask = Ask::Stop { item };
        self.engine_stage.push(Event::Ask { reply_to: ReplyTo::new(Token::new(name)), person: PEOPLE[0], ask });
    }

    /// Sends `event` up the channel to the engine, after what went before it.
    pub(super) fn send_up(&mut self, event: Event) {
        let Some(channel) = self.channel else { return };
        let at = self.now.saturating_add(self.draw(self.settings.network)).max(self.up_lane);
        self.up_lane = at;
        self.schedule(at, Delivery::Engine { channel: Some(channel), event });
    }

    /// What the engine asked for, carried by its protocol layer.
    pub(super) fn engine_request(&mut self, request: Request) {
        self.log(&format!("engine -> {}", describe_request(&request)));
        match request {
            Request::Forge { call, repository, op, payload } => {
                self.stats.forge_calls += 1;
                self.owned.open(call, ());
                let (asked, op) = translate::op(op, payload.as_ref(), self.settings.engine.forge.page);
                let name = self.wire.name();
                let deadline = self.send_at(self.now.saturating_add(self.settings.timeout), Delivery::Timeout(name));
                self.calls.open(name, Out { call, asked, deadline, expired: false });
                let event = forge::Event::Call {
                    reply_to: ReplyTo::new(Token::new(name)),
                    user: deployment::ENGINE,
                    repository: deployment::name(repository).into(),
                    op,
                };
                forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
                self.drain_forge();
            }
            Request::Assign { channel, assignment } => {
                if self.channel == Some(channel) {
                    self.assign(assignment);
                }
            }
            request @ (Request::Inbound { channel, .. }
            | Request::Cancel { channel, .. }
            | Request::Relayed { channel, .. }
            | Request::Acknowledge { channel, .. }) => {
                if self.channel == Some(channel)
                    && let Some(event) = crate::protocol::down(&request)
                {
                    self.send_down(event);
                }
            }
            Request::Refuse { channel } => {
                if self.channel == Some(channel) {
                    self.channel = None;
                    self.send(Delivery::Worker(temper_worker_domain::Event::Lost));
                }
            }
            Request::Reply { to, reply } => {
                let item = self.asks.end(to.into_token().raw());
                if reply == Reply::Done {
                    self.stats.stopped += 1;
                }
                self.log(&format!("the engine answers the stop of {item:?}: {reply:?}"));
            }
            Request::Deliver { .. } | Request::Ended { .. } => {
                unreachable!("nobody watches a run in this world")
            }
            Request::Store { owner, op } => {
                self.stores.open(owner, ());
                let (stored, after) = self.store.apply(op);
                if stored == engine::Stored::Failed {
                    self.stats.store_failed += 1;
                }
                self.send_at(self.now.saturating_add(after), Delivery::Stored { owner, stored });
            }
        }
    }

    /// The engine's protocol layer gives up on its call `name`.
    pub(super) fn timed_out(&mut self, name: u64) {
        let Some(out) = self.calls.get_mut(name) else { return };
        out.expired = true;
        let call = out.call;
        self.stats.timed_out += 1;
        self.answer_engine(call, Err(engine_api::Error::Timeout), Box::new([]));
    }

    /// What the forge emitted: replies to calls, and webhooks.
    pub(super) fn drain_forge(&mut self) {
        while let Some(request) = self.forge_out.pop() {
            self.forge_request(request);
        }
    }

    fn forge_request(&mut self, request: forge::Request) {
        match request {
            forge::Request::Reply { to, result } => self.reply(to.into_token().raw(), result),
            forge::Request::Hook { repository, change: _, number, branch, commit } => {
                let Some(repository) = deployment::index(&repository) else { return };
                self.stats.hints += 1;
                let commit = commit.map(translate::commit);
                let event = Event::Hint { repository, item: number, commit, branch };
                self.engine_stage.push(event);
            }
        }
    }

    /// The forge answered the call `name`: the engine's, or the reviewer's.
    fn reply(&mut self, name: u64, result: Result<temper_forge_domain::api::Answer, temper_forge_domain::api::Error>) {
        if self.calls.contains(name) {
            let out = self.calls.end(name);
            if out.expired {
                return;
            }
            self.wire.withdraw(out.deadline);
            let now = forge::time(&self.settings.forge, self.now);
            let (bodies, page) = translate::bodies(&result);
            let limits = &self.settings.engine.forge;
            let mut answer = temper_engine_domain_forge_tests::translate::answer(out.asked, result, limits, now);
            let decoded = translate::decode(&mut answer, &bodies, page);
            let limited = match &answer {
                Err(error) => match error {
                    engine_api::Error::RateLimited { .. } => true,
                    engine_api::Error::Unavailable
                    | engine_api::Error::Timeout
                    | engine_api::Error::Forbidden
                    | engine_api::Error::Missing
                    | engine_api::Error::TooLarge
                    | engine_api::Error::Empty
                    | engine_api::Error::Full
                    | engine_api::Error::Exists
                    | engine_api::Error::NothingToMerge
                    | engine_api::Error::Closed
                    | engine_api::Error::Stale
                    | engine_api::Error::Conflict
                    | engine_api::Error::Protected
                    | engine_api::Error::Circular => false,
                },
                Ok(_) => false,
            };
            self.stats.limited += u32::from(limited);
            self.answer_engine(out.call, answer, decoded);
            return;
        }
        let (repository, number, head) = self.theirs.end(name);
        self.log(&format!("the reviewer's call answered {}", if result.is_ok() { "ok" } else { "failed" }));
        self.reviewer.reviewed(repository, number, head);
    }

    /// Terminal for the engine's call `call`.
    fn answer_engine(
        &mut self,
        call: Token,
        result: Result<engine_api::Answer, engine_api::Error>,
        decoded: Box<[engine::Decoded]>,
    ) {
        self.owned.end(call);
        self.engine_stage.push(Event::Answered { call, result, decoded });
    }

    /// What the fake forge did: the mirror and the referees see it.
    pub(super) fn observe_forge(&mut self) {
        while let Some(observation) = self.forge.pop_observation() {
            self.mirror.observe(&observation);
            match &observation {
                Observation::Moved { repository, branch, from, to, by } => {
                    self.moved(repository, branch, *from, *to, *by);
                }
                Observation::Merged { repository, number, base, head, commit, by: _ } => {
                    self.stats.merged += 1;
                    self.log(&format!("the forge merges #{number} at {head} into {commit}"));
                    let seen = self.merged_seen(repository, base, *head, *commit);
                    self.observe(seen);
                }
                Observation::Wiki { .. }
                | Observation::Deleted { .. }
                | Observation::Opened { .. }
                | Observation::Closed { .. }
                | Observation::Reopened { .. }
                | Observation::Labelled { .. }
                | Observation::Revised { .. }
                | Observation::Depends { .. }
                | Observation::Requested { .. }
                | Observation::Defined { .. }
                | Observation::Commented { .. }
                | Observation::Edited { .. }
                | Observation::Removed { .. }
                | Observation::Reviewed { .. }
                | Observation::Reported { .. }
                | Observation::Rejected { .. } => {}
                Observation::Refused { what, number, error, by, .. } => {
                    self.log(&format!("the forge refuses {what:?} on {number:?} by {by}: {error:?}"));
                    self.stats.conflicts += u32::from(*error == forge::api::Error::Conflict);
                    assert!(
                        *error != forge::api::Error::Full,
                        "seed {}: the forge has room for everything the world makes, yet refused {what:?} on \
                         {number:?} by {by} for want of it",
                        self.settings.seed
                    );
                }
            }
            self.observe(Seen::Forge(observation.clone()));
            self.observe_engine(engine_referee::Seen::Forge(observation));
        }
        assert_eq!(self.forge.observations_lost(), 0, "the world drains the forge's observations as they come");
    }

    /// The referee sees a move of a branch of the forge, as the forge
    /// observed it, each a fast-forward, with the commits it brings onto its
    /// branch: the worker's pushes, another party's, and the engine's merges.
    fn moved(&mut self, repository: &[u8], branch: &[u8], from: Option<u64>, to: u64, by: u64) {
        if let Some(from) = from {
            assert!(
                self.forge.is_ancestor(from, to),
                "{}: {} moved only by a fast-forward",
                String::from_utf8_lossy(repository),
                String::from_utf8_lossy(branch)
            );
        }
        let mut brought = Vec::new();
        let mut next = Some(to);
        while let Some(commit) = next
            && Some(commit) != from
        {
            brought.push(commit);
            next = self.forge.object(commit).expect("a commit of the store").parent;
        }
        let tree = shared::tree(&self.forge, to);
        let seen = Seen::Moved {
            remote: repository.to_vec(),
            branch: branch.to_vec(),
            tip: to,
            brought,
            tree,
            other: by == OTHER,
        };
        self.observe(seen);
    }

    /// What the referee sees of a merge of `head` onto `base` of
    /// `repository`, as `commit`: the files the head changed since it forked
    /// from where `base` was, as the head has them, and the merge's.
    fn merged_seen(&self, repository: &[u8], base: &[u8], head: u64, commit: u64) -> Seen {
        let onto = self.forge.object(commit).expect("a commit of the store").parent.expect("a merge has a parent");
        let mut fork = onto;
        while !self.forge.is_ancestor(fork, head) {
            fork = self.forge.object(fork).expect("a commit of the store").parent.expect("the two share a root");
        }
        let before = shared::tree(&self.forge, fork);
        let mut changed = shared::tree(&self.forge, head);
        changed.retain(|path, content| before.get(path) != Some(content));
        Seen::Merged {
            remote: repository.to_vec(),
            base: base.to_vec(),
            head,
            changed,
            merged: shared::tree(&self.forge, commit),
        }
    }

    /// Another party moves `branch` of `remote`, making it first if it is
    /// nowhere yet, from the default branch, unless the forge refuses.
    pub(super) fn advance(&mut self, remote: &[u8], branch: &[u8]) {
        let content = format!("another party, {}\n", self.stats.advanced + 1);
        let moved = self.direct(OTHER, true).advance(remote, branch, b"OTHER.md", content.as_bytes());
        self.route_stray();
        if moved {
            self.stats.advanced += 1;
            self.log(&format!("another party moves {}", String::from_utf8_lossy(branch)));
        }
        self.observe_forge();
    }
}

/// The engine's `event`, for the trace.
pub(super) fn describe_event(event: &Event) -> String {
    match event {
        Event::Answered { call, result, decoded } => {
            let result = match result {
                Ok(_) => "ok".to_owned(),
                Err(error) => format!("{error:?}"),
            };
            format!("answered {} {result} ({} decoded)", call.raw(), decoded.len())
        }
        Event::Told { item, attempt, kind, .. } => format!("told {item:?}#{attempt} {kind:?}"),
        Event::Hint { .. }
        | Event::Hello { .. }
        | Event::Lost { .. }
        | Event::Answer { .. }
        | Event::Relay { .. }
        | Event::Bounced { .. }
        | Event::Ask { .. }
        | Event::Unwatch { .. }
        | Event::Delivered { .. }
        | Event::Stored { .. } => format!("{event:?}"),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Assign { channel, assignment } => {
            format!("assign {:?}#{} on {}", assignment.item, assignment.attempt, channel.raw())
        }
        Request::Forge { call, repository, op, .. } => format!("forge {} on {repository}: {op:?}", call.raw()),
        Request::Store { owner, .. } => format!("store {}", owner.raw()),
        Request::Inbound { .. }
        | Request::Cancel { .. }
        | Request::Relayed { .. }
        | Request::Acknowledge { .. }
        | Request::Refuse { .. }
        | Request::Reply { .. }
        | Request::Deliver { .. }
        | Request::Ended { .. } => format!("{request:?}"),
    }
}
