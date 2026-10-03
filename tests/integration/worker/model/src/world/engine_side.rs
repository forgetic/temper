//! The engine's stage, and what it asks of the world: the forge, through its
//! protocol layer as the engine's world plays it; the store; people's
//! answers; and the worker, through the channel ([`super::network`]), its
//! requests translated as the protocol layers would ([`crate::protocol`]).
//! And the people who give it work, the forge they look at, and what both
//! referees see of it.

use temper_engine_model::forge::api as engine_api;
use temper_engine_model::{self as engine, Ask, Fact, Item, Reply, Request};
use temper_engine_model_tests::codec;
use temper_engine_model_tests::deployment::{self, ENGINE, PEOPLE, REPOSITORIES};
use temper_engine_model_tests::people::{self, Asker};
use temper_engine_model_tests::referee::Seen as Told;
use temper_engine_model_tests::translate;
use temper_forge_model::api as forge_api;
use temper_forge_model::{self as forge, Observation};
use temper_lib::{ReplyTo, Token};
use temper_worker_model::Event;

use super::{Asking, Delivery, Out, Theirs, World};
use crate::protocol;
use crate::referee::Seen;

/// Who stops runs: a person who may write to every repository.
const STOPPER: u64 = PEOPLE[1];

impl World {
    pub(super) fn run_engine(&mut self) {
        while self.desk.has_room() && self.engine.is_ready() {
            engine::resume(&mut self.engine, &self.desk.env, &mut self.desk.out);
        }
        while let Some(event) = self.desk.next_event() {
            self.log(format!("engine <- {}", describe(&event)));
            engine::step(&mut self.engine, &self.desk.env, event, &mut self.desk.out);
        }
        while self.desk.has_room() && self.engine.is_due(self.now) {
            engine::fire(&mut self.engine, &self.desk.env, &mut self.desk.out);
        }
        while let Some(request) = self.desk.out.pop() {
            self.request(request);
        }
        while let Some(fact) = self.engine.pop_fact() {
            self.stats.engine_facts += 1;
            match fact {
                Fact::Work { .. }
                | Fact::Forge { .. }
                | Fact::Fleet { .. }
                | Fact::Brief { .. }
                | Fact::Notes { .. }
                | Fact::Views { .. }
                | Fact::Loaded
                | Fact::Mangled { .. }
                | Fact::Untracked { .. }
                | Fact::Ruled { .. } => {}
            }
        }
        self.engine.reclaim();
    }

    /// What the engine asked for.
    fn request(&mut self, request: Request) {
        self.log(format!("engine -> {}", describe_request(&request)));
        match request {
            Request::Forge { call, repository, op, payload } => {
                self.owned.open(call, ());
                let (asked, op) = translate::op(op, payload.as_ref(), self.settings.engine.forge.page);
                let name = self.wire.name();
                let deadline = self.send(self.now.saturating_add(self.settings.timeout), Delivery::Deadline(name));
                self.calls.open(name, Out { call, asked, deadline, expired: false });
                let reply_to = ReplyTo::new(Token::new(name));
                let repository = deployment::name(repository).into();
                let event = forge::Event::Call { reply_to, user: ENGINE, repository, op };
                self.forge_call(event);
            }
            Request::Assign { channel, assignment } => {
                let (item, attempt) = (assignment.item, assignment.attempt);
                let names = World::names(item, attempt);
                let brief = codec::brief_of(&assignment.charter);
                let (assignment, repositories) = protocol::assignment(&assignment);
                self.assigned.insert(names, repositories);
                self.end("assigned");
                if self.first.insert((item, attempt)) {
                    let live = self.up
                        && self.attempts.iter().any(|(&(run, other), record)| {
                            run == names.0 && other != names.1 && !record.refused && record.answer.is_none()
                        });
                    self.stories.observe(self.now, Told::Assigned { item, attempt, live, brief }, &mut Vec::new());
                    self.stories.assert_holding(self.settings.seed);
                    if self.is_change(item) && !self.stopping.contains(&item) && self.rng.chance(self.settings.stops) {
                        self.stopping.insert(item);
                        let at = self.now.saturating_add(self.settings.stop_after.draw(&mut self.rng));
                        self.send(at, Delivery::Stop(item));
                    }
                }
                self.hosting.observe(self.now, Seen::Assigned { names }, &mut Vec::new());
                self.send_down(channel, Event::Assign { assignment }, false);
            }
            Request::Inbound { channel, item, attempt, event } => {
                let names = World::names(item, attempt);
                let place = self.places.entry(names).or_default();
                let framed = protocol::framed_event(*place, names, &event);
                *place += 1;
                self.end("inbound");
                self.send_down(channel, Event::Inbound { run: names.0, attempt: names.1, event: framed }, false);
            }
            Request::Cancel { channel, item, attempt } => {
                let (run, attempt) = World::names(item, attempt);
                self.end("cancelled");
                self.send_down(channel, Event::Cancel { run, attempt }, true);
            }
            Request::Relayed { channel, item, attempt, call, served } => {
                let names = World::names(item, attempt);
                self.end("relayed");
                self.hosting.observe(self.now, Seen::Relayed { names, call }, &mut Vec::new());
                self.hosting.assert_holding(self.settings.seed);
                let answer = protocol::served(&served);
                self.send_down(channel, Event::Relayed { run: names.0, attempt: names.1, call, answer }, true);
            }
            Request::Acknowledge { channel, item, attempt } => {
                let names = World::names(item, attempt);
                self.end("acknowledged");
                self.hosting.observe(self.now, Seen::Acknowledged { names }, &mut Vec::new());
                self.hosting.assert_holding(self.settings.seed);
                self.send_down(channel, Event::Acknowledged { run: names.0, attempt: names.1 }, true);
            }
            Request::Refuse { channel } => self.refuse(channel),
            Request::Reply { to, reply } => self.replied(to.into_token().raw(), reply),
            Request::Deliver { watcher, .. } => {
                let at = self.now.saturating_add(self.settings.network.hop.draw(&mut self.rng));
                self.send(at, Delivery::Engine(engine::Event::Delivered { watcher, done: true }));
            }
            Request::Ended { .. } => {}
            Request::Store { owner, op } => {
                self.stores.open(owner, ());
                let (stored, after) = self.store.apply(op);
                self.stores.end(owner);
                let event = engine::Event::Stored { owner, stored };
                self.send(self.now.saturating_add(after), Delivery::Engine(event));
            }
        }
    }

    /// The engine answered the ask `name`.
    fn replied(&mut self, name: u64, reply: Reply) {
        match self.asks.end(name) {
            Asking::People(asker, message, accept) => {
                let messaged = message.is_some();
                if let Some(item) = accept {
                    self.stories.observe(self.now, Told::Accepted { item, reply }, &mut Vec::new());
                }
                if reply == Reply::Done
                    && let Some((item, key, text)) = message
                {
                    self.stories.observe(self.now, Told::Messaged { item, key, text }, &mut Vec::new());
                    self.stories.assert_holding(self.settings.seed);
                }
                if let Asker::Caretaker(_) = asker
                    && reply == Reply::Done
                {
                    self.end("released");
                }
                self.people.replied(asker, reply, messaged);
            }
            Asking::Stopper(item) => {
                self.stopping.remove(&item);
                if reply == Reply::Done {
                    self.end("stopped");
                }
            }
        }
    }

    /// The protocol layer's deadline for the engine's call `name` passed.
    pub(super) fn deadline(&mut self, name: u64) {
        let Some(out) = self.calls.get_mut(name) else { return };
        out.expired = true;
        let call = out.call;
        self.end("timed out");
        self.answer(call, Err(engine_api::Error::Timeout), Box::new([]));
    }

    /// Terminal for the engine's call `call`.
    fn answer(
        &mut self,
        call: Token,
        result: Result<engine_api::Answer, engine_api::Error>,
        decoded: Box<[engine::Decoded]>,
    ) {
        self.owned.end(call);
        self.desk.push(engine::Event::Answered { call, result, decoded });
    }

    /// A call to the forge, made now.
    fn forge_call(&mut self, event: forge::Event) {
        let env = temper_lib::Env { now: self.now, limits: self.settings.forge };
        forge::step(&mut self.forge, &env, event, &mut self.forge_out);
        self.drain_forge();
    }

    /// What the forge emitted: replies to calls, and webhooks.
    pub(super) fn drain_forge(&mut self) {
        while let Some(request) = self.forge_out.pop() {
            match request {
                forge::Request::Reply { to, result } => self.forge_reply(to.into_token().raw(), result),
                forge::Request::Hook { repository, change: _, number, branch, commit } => {
                    let Some(repository) = deployment::index(&repository) else { continue };
                    let commit = commit.map(translate::commit);
                    self.desk.push(engine::Event::Hint { repository, item: number, commit, branch });
                }
            }
        }
    }

    /// The forge answered the call `name`.
    fn forge_reply(&mut self, name: u64, result: Result<forge_api::Answer, forge_api::Error>) {
        if self.calls.contains(name) {
            let out = self.calls.end(name);
            if out.expired {
                return;
            }
            self.withdraw(out.deadline);
            let now = forge::time(&self.settings.forge, self.now);
            let (bodies, page) = translate::bodies(&result);
            let limits = &self.settings.engine.forge;
            let mut answer = temper_engine_model_forge_tests::translate::answer(out.asked, result, limits, now);
            let decoded = translate::decode(&mut answer, &bodies, page);
            self.answer(out.call, answer, decoded);
            return;
        }
        let made = result.is_ok();
        match self.theirs.end(name) {
            Theirs::Person { tale } => {
                if let Some(tale) = tale {
                    self.people.called(tale, made);
                }
            }
            Theirs::Review { repository, number, head } => self.people.reviewed(repository, number, head),
        }
    }

    /// What the fake forge did: the mirror and the referees see it.
    pub(super) fn observe_forge(&mut self) {
        while let Some(observation) = self.forge.pop_observation() {
            self.mirror.observe(&observation);
            if let Some((repository, number, lifecycle)) = record_written(&observation) {
                self.hosting.observe(self.now, Seen::Recorded { repository, number, lifecycle }, &mut Vec::new());
                self.hosting.assert_holding(self.settings.seed);
            }
            match &observation {
                Observation::Merged { .. } => self.end("merged"),
                Observation::Reviewed { .. } => self.end("reviewed"),
                Observation::Closed { repository, number, .. } => {
                    let closed = (0..self.settings.stories.len()).any(|tale| {
                        self.people.item(tale).is_some_and(|item| {
                            deployment::name(item.repository) == &**repository && item.number == *number
                        })
                    });
                    if closed {
                        self.end("story closed");
                    }
                }
                Observation::Moved { repository, from: Some(from), to, by, .. } if *by == deployment::WORKER => {
                    assert!(
                        self.forge.is_ancestor(*from, *to),
                        "{}: the worker moves a branch only by a fast-forward",
                        String::from_utf8_lossy(repository)
                    );
                }
                Observation::Wiki { .. }
                | Observation::Moved { .. }
                | Observation::Deleted { .. }
                | Observation::Opened { .. }
                | Observation::Reopened { .. }
                | Observation::Labelled { .. }
                | Observation::Revised { .. }
                | Observation::Depends { .. }
                | Observation::Requested { .. }
                | Observation::Defined { .. }
                | Observation::Commented { .. }
                | Observation::Edited { .. }
                | Observation::Removed { .. }
                | Observation::Reported { .. }
                | Observation::Refused { .. }
                | Observation::Rejected { .. } => {}
            }
            self.stories.observe(self.now, Told::Forge(observation), &mut Vec::new());
            self.stories.assert_holding(self.settings.seed);
        }
        assert_eq!(self.forge.observations_lost(), 0, "the world drains the forge's observations as they come");
    }

    /// People look at the forge, and act.
    pub(super) fn look(&mut self) {
        let mut acts = Vec::new();
        self.people.act(&self.mirror, &mut acts);
        for act in acts {
            self.person(act);
        }
        for tale in 0..self.settings.stories.len() {
            if let Some(item) = self.people.item(tale) {
                self.stories.observe(self.now, Told::Story { tale, item }, &mut Vec::new());
            }
        }
        if !self.people.is_done(&self.mirror) {
            let gap = self.settings.people.draw(&mut self.rng);
            self.send(self.now.saturating_add(gap), Delivery::People);
        }
    }

    /// A person acts.
    fn person(&mut self, act: people::Act) {
        match act {
            people::Act::Ask { asker, person, ask } => {
                let message = match &ask {
                    Ask::Message { item, key, message } => Some((*item, key.to_vec(), message.to_vec())),
                    Ask::Open { .. }
                    | Ask::Accept { .. }
                    | Ask::Reject { .. }
                    | Ask::Stop { .. }
                    | Ask::Release { .. }
                    | Ask::Watch { .. } => None,
                };
                let accept = match &ask {
                    Ask::Accept { item } => Some(*item),
                    Ask::Open { .. }
                    | Ask::Message { .. }
                    | Ask::Reject { .. }
                    | Ask::Stop { .. }
                    | Ask::Release { .. }
                    | Ask::Watch { .. } => None,
                };
                self.ask(Asking::People(asker, message, accept), person, ask);
            }
            people::Act::Forge { tale, user, repository, op } => {
                self.log(format!("person {user} calls {op:?}"));
                let theirs = match &op {
                    forge_api::Op::Write(forge_api::Write::Review { number, .. }) => {
                        let issue = self.mirror.issue(REPOSITORIES[repository], *number).expect("a pull request");
                        let head = issue.pull.as_ref().expect("a pull request").commit;
                        Theirs::Review { repository, number: *number, head }
                    }
                    forge_api::Op::Read(_) | forge_api::Op::Write(_) | forge_api::Op::Git(_) => Theirs::Person { tale },
                };
                let name = self.wire.name();
                self.theirs.open(name, theirs);
                let reply_to = ReplyTo::new(Token::new(name));
                let event = forge::Event::Call { reply_to, user, repository: REPOSITORIES[repository].into(), op };
                self.forge_call(event);
            }
        }
    }

    /// A person asks the engine through its web.
    fn ask(&mut self, asking: Asking, person: u64, ask: Ask) {
        let name = self.wire.name();
        self.asks.open(name, asking);
        self.log(format!("person {person} asks {ask:?}"));
        self.desk.push(engine::Event::Ask { reply_to: ReplyTo::new(Token::new(name)), person, ask });
    }

    /// Whether the item carries a change: a run a person may stop, whose
    /// item a release makes due again. (A session stopped waits, once
    /// released, for its person's next message, which no story sends.)
    fn is_change(&self, item: Item) -> bool {
        let record = self.mirror.record(deployment::name(item.repository), item.number);
        record.is_some_and(|record| match record.step.step.work {
            engine::plan::Work::Change(_) => true,
            engine::plan::Work::Agent(_) | engine::plan::Work::Wait(_) | engine::plan::Work::Session(_) => false,
        })
    }

    /// A person stops the item's run.
    pub(super) fn stop_run(&mut self, item: Item) {
        self.ask(Asking::Stopper(item), STOPPER, Ask::Stop { item });
    }
}

/// The record the engine wrote, if `observation` is the engine writing one:
/// the item's, and what it says of its lifecycle.
fn record_written(observation: &Observation) -> Option<(Vec<u8>, u64, engine::work::Lifecycle)> {
    let (repository, number, id, body) = match observation {
        Observation::Commented { repository, number, id, body, by }
        | Observation::Edited { repository, number, id, body, by }
            if *by == ENGINE =>
        {
            (repository, number, id, body)
        }
        Observation::Commented { .. }
        | Observation::Edited { .. }
        | Observation::Moved { .. }
        | Observation::Deleted { .. }
        | Observation::Opened { .. }
        | Observation::Closed { .. }
        | Observation::Reopened { .. }
        | Observation::Labelled { .. }
        | Observation::Revised { .. }
        | Observation::Depends { .. }
        | Observation::Requested { .. }
        | Observation::Defined { .. }
        | Observation::Removed { .. }
        | Observation::Reviewed { .. }
        | Observation::Reported { .. }
        | Observation::Merged { .. }
        | Observation::Refused { .. }
        | Observation::Rejected { .. }
        | Observation::Wiki { .. } => return None,
    };
    match codec::comment(*id, body)? {
        engine::Decoded::Record { record, .. } => Some((repository.to_vec(), *number, record.lifecycle)),
        engine::Decoded::Outcome { .. } | engine::Decoded::Page { .. } => None,
    }
}

fn describe(event: &engine::Event) -> String {
    match event {
        engine::Event::Answered { call, result, decoded } => {
            let result = match result {
                Ok(answer) => format!("{:?}", std::mem::discriminant(answer)),
                Err(error) => format!("{error:?}"),
            };
            format!("answered {} {result} ({} decoded)", call.raw(), decoded.len())
        }
        engine::Event::Hint { .. }
        | engine::Event::Hello { .. }
        | engine::Event::Lost { .. }
        | engine::Event::Answer { .. }
        | engine::Event::Relay { .. }
        | engine::Event::Bounced { .. }
        | engine::Event::Told { .. }
        | engine::Event::Ask { .. }
        | engine::Event::Unwatch { .. }
        | engine::Event::Delivered { .. }
        | engine::Event::Stored { .. } => format!("{event:?}"),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Assign { channel, assignment } => {
            format!("assign {:?}#{} on {}", assignment.item, assignment.attempt, channel.raw())
        }
        Request::Forge { call, repository, op, .. } => format!("forge {} {repository} {op:?}", call.raw()),
        Request::Inbound { .. }
        | Request::Cancel { .. }
        | Request::Relayed { .. }
        | Request::Acknowledge { .. }
        | Request::Refuse { .. }
        | Request::Reply { .. }
        | Request::Deliver { .. }
        | Request::Ended { .. }
        | Request::Store { .. } => format!("{request:?}"),
    }
}
