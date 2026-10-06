//! Deterministic shell loop, latency, faults and a scripted durable engine.
use crate::engine::Engine;
use crate::referee::{Rules, Seen, is_link_live, is_refused, observe_at};
use crate::scenario::Scenario;
use crate::tab::Tab;
use skein_lib::{Duration, Rng, Time, Token, Wall};
use std::collections::{BTreeMap, BTreeSet};
use temper_fake_person::RandomPerson;
use temper_fake_person::{Doing, Next, Person as ScriptedPerson, TreeFace, dom_event};
use temper_web_domain::{
    Address, Answer, Ask, Backoff, Change, Event, Key, Outcome, Query, ReadResult, Request, Snapshot, StreamEnd,
    StreamEvent, Watch,
};
use temper_web_view::DomEvent;
use temper_world::Referee;
use temper_world::Verdict;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub seed: u64,
    pub domain: temper_web_domain::Limits,
    pub view: temper_web_view::Limits,
    pub latency: Duration,
    pub restart_per_mille: u32,
    pub busy_per_mille: u32,
    pub drop_per_mille: u32,
    pub miss_per_mille: u32,
    pub reload_per_mille: u32,
    pub double_per_mille: u32,
    pub signed_out_per_mille: u32,
}

impl Settings {
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        let domain = temper_web_domain::Limits {
            objects: 4,
            requests: 4,
            streams: 2,
            reads: 2,
            window: 8,
            turns: 4,
            tree: 4,
            drafts: 1,
            notices: 8,
            words: 128,
            text: 128,
            streaming: 128,
            backoff: Backoff { first: Duration::from_millis(10), most: Duration::from_millis(80) },
            heartbeat: Duration::from_secs(5),
            linger: Duration::from_millis(100),
            notice: Duration::from_secs(2),
            save: Duration::from_millis(10),
            facts: 8,
            projects: 2,
        };
        let view = temper_web_view::Limits::of(&domain).expect("valid view limits");
        Settings {
            seed,
            domain,
            view,
            latency: Duration::from_millis(5),
            restart_per_mille: 0,
            busy_per_mille: 0,
            drop_per_mille: 0,
            miss_per_mille: 0,
            reload_per_mille: 0,
            double_per_mille: 0,
            signed_out_per_mille: 0,
        }
    }

    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut settings = Self::calm(seed);
        settings.restart_per_mille = 4;
        settings.busy_per_mille = 30;
        settings.drop_per_mille = 10;
        settings.miss_per_mille = 10;
        settings.reload_per_mille = 4;
        settings.double_per_mille = 40;
        settings.signed_out_per_mille = 3;
        settings
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Fault {
    Restart,
    Busy,
    Dropped,
    Missed,
    Reload,
    DoublePress,
    SignedOut,
    Latency,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub faults: BTreeMap<Fault, u64>,
    pub steps: u64,
    pub commits: u64,
}

#[derive(Debug)]
enum Due {
    Deliver(Event),
    Commit { request: Token, key: Key, ask: Ask },
    Read { read: Token, query: Query },
    Snapshot { stream: Token, watch: Watch },
    BackOnline,
}

#[derive(Debug)]
struct Scheduled {
    at: Time,
    serial: u64,
    epoch: u64,
    due: Due,
}

#[derive(Debug)]
pub struct World {
    pub settings: Settings,
    pub now: Time,
    pub tab: Tab,
    pub engine: Engine,
    pub referee: Referee<Rules>,
    pub stats: Stats,
    rng: Rng,
    schedule: Vec<Scheduled>,
    serial: u64,
    sends: BTreeSet<(u64, Token)>,
    watches: BTreeSet<(u64, Token)>,
    watch_kinds: BTreeMap<(u64, Token), Watch>,
    live_watches: BTreeSet<(u64, Token)>,
    watch_live: bool,
    busy_next: bool,
    last_typed: Option<Vec<u8>>,
    trace: Vec<String>,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings, scenario: Scenario) -> World {
        let tab = Tab::new(&settings, settings.seed ^ 0x5a5a, Address::Chats);
        let mut world = World {
            now: Time::ZERO,
            tab,
            engine: Engine::new(scenario),
            referee: Referee::new(Rules::new()),
            stats: Stats::default(),
            rng: Rng::new(settings.seed),
            schedule: Vec::new(),
            serial: 0,
            sends: BTreeSet::new(),
            watches: BTreeSet::new(),
            watch_kinds: BTreeMap::new(),
            live_watches: BTreeSet::new(),
            watch_live: false,
            busy_next: false,
            last_typed: None,
            trace: Vec::new(),
            settings,
        };
        for chat in &world.engine.chats {
            observe_at(&mut world.referee, world.now, Seen::Existing { task: chat.task });
        }
        let requests = world.tab.start(&world.settings, world.now);
        world.route(requests);
        world.observe();
        world
    }

    #[must_use]
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    fn later(&mut self, delay: Duration, epoch: u64, due: Due) {
        if delay != Duration::ZERO {
            *self.stats.faults.entry(Fault::Latency).or_default() += 1;
        }
        self.serial += 1;
        self.schedule.push(Scheduled { at: self.now.saturating_add(delay), serial: self.serial, epoch, due });
    }

    fn route(&mut self, requests: Vec<Request>) {
        for request in requests {
            self.trace.push(format!("{} {request:?}", self.now.as_nanos()));
            match request {
                Request::Save { saved } => self.tab.saved = Some(saved),
                Request::Address { address, push } => self.tab.address(address, push),
                Request::SignIn { then } => {
                    self.engine.signed_in = true;
                    self.tab.address(then, false);
                    self.reload();
                }
                Request::Send { request, key, ask } => {
                    if let Ask::StartChat { words, .. } = &ask
                        && self.last_typed.as_deref() == Some(words.as_ref())
                    {
                        self.last_typed = None;
                    }
                    observe_at(&mut self.referee, self.now, Seen::Submitted { key });
                    let epoch = self.tab.epoch;
                    assert!(self.sends.insert((epoch, request)), "one send per token");
                    if !self.engine.signed_in {
                        self.later(
                            self.settings.latency,
                            epoch,
                            Due::Deliver(Event::Answered { request, answer: Answer::SignedOut }),
                        );
                    } else if !self.engine.online {
                        self.later(
                            self.settings.latency,
                            epoch,
                            Due::Deliver(Event::Answered { request, answer: Answer::Unreachable }),
                        );
                    } else if self.busy_next || self.rng.chance(self.settings.busy_per_mille) {
                        self.busy_next = false;
                        *self.stats.faults.entry(Fault::Busy).or_default() += 1;
                        self.later(
                            self.settings.latency,
                            epoch,
                            Due::Deliver(Event::Answered { request, answer: Answer::Busy }),
                        );
                    } else {
                        self.later(self.settings.latency, epoch, Due::Commit { request, key, ask });
                    }
                }
                Request::Read { read, query } => {
                    self.later(self.settings.latency, self.tab.epoch, Due::Read { read, query });
                }
                Request::Open { stream, watch } => {
                    let epoch = self.tab.epoch;
                    assert!(self.watches.insert((epoch, stream)), "one open per watch");
                    self.watch_kinds.insert((epoch, stream), watch);
                    self.watch_live = false;
                    self.later(Duration::ZERO, epoch, Due::Deliver(Event::Opened { stream }));
                    self.later(self.settings.latency, epoch, Due::Snapshot { stream, watch });
                }
                Request::Close { stream } => {
                    let epoch = self.tab.epoch;
                    assert!(
                        self.watches.remove(&(epoch, stream)),
                        "duplicate Close for {stream:?}; trace: {:?}",
                        self.trace.iter().rev().take(8).collect::<Vec<_>>()
                    );
                    self.watch_kinds.remove(&(epoch, stream));
                    self.live_watches.remove(&(epoch, stream));
                    self.watch_live = !self.watches.is_empty() && self.watches == self.live_watches;
                    self.later(Duration::ZERO, epoch, Due::Deliver(Event::Ended { stream, end: StreamEnd::Closed }));
                }
            }
        }
    }

    fn observe(&mut self) {
        let tree = self.tab.view.tree();
        let address = self.tab.address;
        observe_at(
            &mut self.referee,
            self.now,
            Seen::Visible {
                address,
                link_live: is_link_live(tree),
                watch_live: self.watch_live,
                refused: is_refused(tree),
            },
        );
        if let Verdict::Failed(failure) = self.referee.verdict() {
            panic!("seed {}: {failure}; trace: {:?}", self.settings.seed, self.trace);
        }
    }

    fn deliver(&mut self, event: Event) {
        match &event {
            Event::Answered { request, .. } => {
                assert!(self.sends.remove(&(self.tab.epoch, *request)), "send terminal once");
            }
            Event::Ended { stream, .. } => {
                self.watches.remove(&(self.tab.epoch, *stream));
                self.watch_kinds.remove(&(self.tab.epoch, *stream));
                self.live_watches.remove(&(self.tab.epoch, *stream));
                self.watch_live = !self.watches.is_empty() && self.watches == self.live_watches;
            }
            Event::Streamed { stream, event: StreamEvent::Snapshot(_), .. } => {
                self.live_watches.insert((self.tab.epoch, *stream));
                self.watch_live = self.watches == self.live_watches;
            }
            Event::Streamed { stream, event: StreamEvent::Missed { .. }, .. } => {
                self.live_watches.remove(&(self.tab.epoch, *stream));
                self.watch_live = false;
            }
            Event::Start { .. }
            | Event::Went { .. }
            | Event::Act { .. }
            | Event::Read { .. }
            | Event::Opened { .. }
            | Event::Streamed { .. } => {}
        }
        let requests = self.tab.step(&self.settings, self.now, event);
        self.route(requests);
        self.observe();
    }

    fn process(&mut self, scheduled: Scheduled) {
        let epoch = scheduled.epoch;
        match scheduled.due {
            Due::Deliver(event) => {
                if epoch == self.tab.epoch {
                    match &event {
                        Event::Answered { request, .. } if !self.sends.contains(&(epoch, *request)) => return,
                        Event::Opened { stream } if !self.watches.contains(&(epoch, *stream)) => return,
                        Event::Streamed { stream, .. } if !self.watches.contains(&(epoch, *stream)) => return,
                        Event::Start { .. }
                        | Event::Went { .. }
                        | Event::Act { .. }
                        | Event::Answered { .. }
                        | Event::Read { .. }
                        | Event::Opened { .. }
                        | Event::Streamed { .. }
                        | Event::Ended { .. } => {}
                    }
                    self.deliver(event);
                }
            }
            Due::Commit { request, key, ask } => {
                if !self.engine.online {
                    return;
                }
                let previous = self.engine.creations;
                let prior_decisions = self.engine.decision_count;
                let decided_task = match &ask {
                    Ask::Decide { waiting: temper_web_domain::Waiting::Escalation { task }, .. } => Some(*task),
                    Ask::StartChat { .. } => None,
                };
                let answer = self.engine.commit(key, ask, Wall::from_nanos(self.now.as_nanos()));
                if self.engine.creations > previous {
                    self.stats.commits += 1;
                    if let Answer::Done(Outcome::Started { task }) = answer {
                        observe_at(&mut self.referee, self.now, Seen::Durable { key, task });
                    }
                }
                if self.engine.decision_count > prior_decisions
                    && let Some(task) = decided_task
                {
                    self.notify_task(task);
                }
                if epoch == self.tab.epoch && self.sends.contains(&(epoch, request)) {
                    self.later(self.settings.latency, epoch, Due::Deliver(Event::Answered { request, answer }));
                }
            }
            Due::Read { read, query } => {
                if epoch != self.tab.epoch {
                    return;
                }
                let result = match query {
                    Query::Chats { project, .. } => self.engine.read_chats(project, self.settings.domain.window),
                    Query::Escalation { task } => {
                        ReadResult::Escalation(self.engine.task_snapshot(task).and_then(|snapshot| snapshot.escalation))
                    }
                    Query::Result { task } => {
                        ReadResult::Result(self.engine.task_snapshot(task).and_then(|snapshot| snapshot.result))
                    }
                };
                self.deliver(Event::Read { read, result });
            }
            Due::Snapshot { stream, watch } => {
                if epoch != self.tab.epoch || !self.watches.contains(&(epoch, stream)) {
                    return;
                }
                if !self.engine.signed_in {
                    self.deliver(Event::Ended { stream, end: StreamEnd::SignedOut });
                } else if !self.engine.online {
                    self.deliver(Event::Ended { stream, end: StreamEnd::Dropped });
                } else {
                    let snapshot = match watch {
                        Watch::Person => Some(Snapshot::Person(self.engine.snapshot())),
                        Watch::Task { number } => self.engine.task_snapshot(number).map(Snapshot::Task),
                    };
                    if let Some(snapshot) = snapshot {
                        self.deliver(Event::Streamed { stream, event: StreamEvent::Snapshot(snapshot) });
                    } else {
                        self.deliver(Event::Ended { stream, end: StreamEnd::Gone });
                    }
                }
            }
            Due::BackOnline => self.engine.online = true,
        }
    }

    pub fn advance(&mut self, span: Duration) {
        let target = self.now.saturating_add(span);
        let mut count = 0_u32;
        loop {
            let next_schedule = self.schedule.iter().filter(|item| item.at <= target).map(|item| item.at).min();
            let next_timer = self.tab.domain.next_deadline().filter(|at| *at <= target);
            let next = [next_schedule, next_timer].into_iter().flatten().min();
            let Some(next) = next else {
                break;
            };
            self.now = next;
            if let Some(index) = self
                .schedule
                .iter()
                .enumerate()
                .filter(|(_, item)| item.at <= self.now)
                .min_by_key(|(_, item)| (item.at, item.serial))
                .map(|(index, _)| index)
            {
                let item = self.schedule.remove(index);
                self.process(item);
            } else {
                let requests = self.tab.fire(&self.settings, self.now);
                self.route(requests);
                self.observe();
            }
            count += 1;
            assert!(count < 10_000, "world loop settles");
        }
        self.now = target;
        let mut stimuli = Vec::new();
        if self.referee.is_due(self.now) {
            self.referee.fire(self.now, &mut stimuli);
            assert!(stimuli.is_empty(), "no scheduled referee stimuli in this story");
        }
        self.referee.assert_holding(self.settings.seed);
    }

    pub fn act(&mut self, event: DomEvent) {
        if let DomEvent::Input { text, .. } = &event {
            self.last_typed = Some(text.to_vec());
        }
        let duplicate = matches!(event, DomEvent::Press { .. }) && self.rng.chance(self.settings.double_per_mille);
        let copy = if duplicate {
            Some(match &event {
                DomEvent::Press { node } => DomEvent::Press { node: *node },
                DomEvent::Input { .. } | DomEvent::Submit { .. } => unreachable!(),
            })
        } else {
            None
        };
        let requests = self.tab.event(&self.settings, self.now, event);
        self.route(requests);
        if let Some(copy) = copy {
            *self.stats.faults.entry(Fault::DoublePress).or_default() += 1;
            let requests = self.tab.event(&self.settings, self.now, copy);
            self.route(requests);
        }
        self.observe();
    }

    pub fn run(&mut self, person: &mut ScriptedPerson) {
        for _ in 0..10_000 {
            let next = person.next(&mut TreeFace::new(self.tab.view.tree()), self.now);
            match next {
                Next::Done => return,
                Next::Failed { step, why } => panic!("person step {step}: {why:?}; trace: {:?}", self.trace),
                Next::Wait { .. } => self.advance(Duration::from_millis(1)),
                Next::Do(doing) => match doing {
                    Doing::Go { address } => {
                        let requests = self.tab.go(&self.settings, self.now, address);
                        self.route(requests);
                        self.observe();
                    }
                    Doing::Reload => self.reload(),
                    action @ (Doing::Press { .. } | Doing::Type { .. } | Doing::Send { .. }) => {
                        self.act(dom_event(action).expect("node action has DOM event"));
                    }
                },
            }
            self.stats.steps += 1;
        }
        panic!("person did not finish; trace: {:?}", self.trace);
    }

    pub fn reload(&mut self) {
        *self.stats.faults.entry(Fault::Reload).or_default() += 1;
        self.watches.clear();
        self.watch_kinds.clear();
        self.live_watches.clear();
        self.watch_live = false;
        self.sends.retain(|(epoch, _)| *epoch != self.tab.epoch);
        let seed = self.rng.next_u64();
        let requests = self.tab.reload(&self.settings, seed, self.now);
        self.route(requests);
        if self.tab.address == Address::Chats
            && let Some(before) = &self.last_typed
        {
            let after = self
                .tab
                .view
                .tree()
                .nodes()
                .iter()
                .find(|node| node.name.as_deref() == Some(b"Start a new chat".as_slice()))
                .and_then(|node| node.value.as_ref())
                .map_or_else(Vec::new, |value| value.text.to_vec());
            observe_at(&mut self.referee, self.now, Seen::Reloaded { before: before.clone(), after });
        }
        self.observe();
    }

    pub fn restart(&mut self, offline: Duration) {
        *self.stats.faults.entry(Fault::Restart).or_default() += 1;
        self.engine.online = false;
        let epoch = self.tab.epoch;
        let sends: Vec<Token> = self.sends.iter().filter(|(e, _)| *e == epoch).map(|(_, token)| *token).collect();
        for token in sends {
            self.deliver(Event::Answered { request: token, answer: Answer::Unreachable });
        }
        let watches: Vec<Token> = self.watches.iter().filter(|(e, _)| *e == epoch).map(|(_, token)| *token).collect();
        for token in watches {
            self.deliver(Event::Ended { stream: token, end: StreamEnd::Dropped });
        }
        observe_at(&mut self.referee, self.now, Seen::WatchLost);
        self.later(offline, epoch, Due::BackOnline);
    }

    pub fn busy_once(&mut self) {
        self.busy_next = true;
    }

    pub fn drop_watch(&mut self) {
        if let Some((_, stream)) = self.watches.iter().find(|(epoch, _)| *epoch == self.tab.epoch).copied() {
            *self.stats.faults.entry(Fault::Dropped).or_default() += 1;
            self.deliver(Event::Ended { stream, end: StreamEnd::Dropped });
            observe_at(&mut self.referee, self.now, Seen::WatchLost);
        }
    }

    pub fn miss_watch(&mut self) {
        if let Some((_, stream)) = self.watches.iter().find(|(epoch, _)| *epoch == self.tab.epoch).copied() {
            *self.stats.faults.entry(Fault::Missed).or_default() += 1;
            self.deliver(Event::Streamed { stream, event: StreamEvent::Missed { count: 1 } });
            observe_at(&mut self.referee, self.now, Seen::WatchLost);
        }
    }

    pub fn expire_signin(&mut self) {
        *self.stats.faults.entry(Fault::SignedOut).or_default() += 1;
        self.engine.signed_in = false;
        let sends: Vec<Token> =
            self.sends.iter().filter(|(e, _)| *e == self.tab.epoch).map(|(_, token)| *token).collect();
        for token in sends {
            self.deliver(Event::Answered { request: token, answer: Answer::SignedOut });
        }
        let watches: Vec<Token> =
            self.watches.iter().filter(|(e, _)| *e == self.tab.epoch).map(|(_, token)| *token).collect();
        for token in watches {
            if self.watches.contains(&(self.tab.epoch, token)) {
                self.deliver(Event::Ended { stream: token, end: StreamEnd::SignedOut });
            }
        }
    }

    pub fn settle(&mut self) {
        self.advance(Duration::from_millis(250));
    }

    fn notify_task(&mut self, task: u64) {
        let Some(snapshot) = self.engine.task_snapshot(task) else {
            return;
        };
        let followers: Vec<_> = self
            .watch_kinds
            .iter()
            .filter_map(|((epoch, stream), watch)| {
                (*watch == Watch::Task { number: task }).then_some((*epoch, *stream))
            })
            .collect();
        for (epoch, stream) in followers {
            self.later(
                self.settings.latency,
                epoch,
                Due::Deliver(Event::Streamed { stream, event: StreamEvent::Change(Change::Task(snapshot.clone())) }),
            );
        }
    }

    pub fn move_task_revision(&mut self, task: u64) {
        self.engine.move_revision(task);
        self.notify_task(task);
    }

    pub fn end_task(&mut self, task: u64, words: &[u8]) {
        self.engine.end(task, words);
        self.notify_task(task);
    }

    pub fn random_step(&mut self, person: &mut RandomPerson) {
        if self.rng.chance(self.settings.restart_per_mille) {
            self.restart(Duration::from_millis(20));
        }
        if self.rng.chance(self.settings.drop_per_mille) {
            self.drop_watch();
        }
        if self.rng.chance(self.settings.miss_per_mille) {
            self.miss_watch();
        }
        if self.rng.chance(self.settings.reload_per_mille) {
            self.reload();
        }
        if self.rng.chance(self.settings.signed_out_per_mille) {
            self.expire_signin();
        }
        if let Some(event) = person.next(self.tab.view.tree()) {
            self.act(event);
        }
        self.advance(Duration::from_millis(1));
    }
}
