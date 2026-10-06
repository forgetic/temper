//! Actual authenticated role asks on the real root and children, restored from
//! genuine held-chat transactions through the shared ordered fake store. The
//! world retains only outside session/request scripts and durable evidence;
//! no worker claim is invented (domain/people.md, section 5.1).

use crate::commits::Store;
use crate::escalation;
use crate::escalation_referee::Story;
use crate::roles_referee::{Referee, Replacement};
use crate::walking;
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_domain::{Delivery, Key, Record, Write, engine};
use temper_engine_domain_accounts as accounts;
use temper_engine_domain_authority as authority;
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;

/// Actual held-chat state at which the previous process stops
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    /// Requester holds Waiting revision one.
    Requester,

    /// Requester passed to the final policy role at revision two
    FinalRole,

    /// Requester rejected, retaining its durable reason
    Rejected,
}

/// Tiny outside scheduling and startup configuration; child capacities remain
/// immutable throughout the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent outside fault and observation switches remain explicit")]
pub struct Settings {
    /// Deterministic real-child seed.
    pub seed: u64,

    /// Genuine previous root story.
    pub base: Base,

    /// Lose one successful role commit completion after atomic application
    /// (domain/engine.md, section 5.3).
    pub cut: bool,

    /// Delay each atomic store application by this many passes
    pub commit_delay: u32,

    /// Delay each captured one-row restore page by this many passes
    pub page_delay: u32,

    /// Drain optional facts or saturate their tiny queues
    pub facts: bool,

    /// Current owner's policy permits Policy as well as Create/Accept
    /// (domain/people.md, section 5.2).
    pub policy: bool,

    /// Actual durable Waiting row reaches checked revision exhaustion before
    /// this fresh process starts.
    pub exhausted: bool,

    /// Start beyond both saved session expiry clocks
    /// (domain/people.md, section 3).
    pub expired: bool,
}

impl Settings {
    /// Calm authenticated requester-held role story
    #[must_use]
    pub const fn calm(seed: u64, base: Base) -> Settings {
        Settings {
            seed,
            base,
            cut: false,
            commit_delay: 0,
            page_delay: 0,
            facts: true,
            policy: true,
            exhausted: false,
            expired: false,
        }
    }
}

#[derive(Clone, Debug)]
struct Input {
    owner: usize,
    key: [u8; 16],
    ask: people::Ask,
}

/// One loop with actual root outputs, paged Store, authenticated outside people,
/// and the independent transaction observer.
#[derive(Debug)]
pub struct World {
    settings: Settings,
    domain: engine::Domain,
    environment: Env<engine::Limits>,
    out: Queue<engine::Request>,
    events: VecDeque<(u32, engine::Event)>,
    inputs: BTreeMap<Token, Input>,
    iteration: u32,
    commit_wait: u32,
    frozen_cut: bool,
    recovering: bool,
    next_right: u64,
    /// Saved actual sign-in identifiers, including deliberately expired ones
    /// (domain/people.md, section 3).
    pub sessions: [u64; 2],

    /// Deployment people created by genuine authenticated sign-ins
    /// (domain/people.md, section 3).
    pub people: [u64; 2],

    /// Genuine held-chat identity.
    pub task: u64,

    /// Shared atomic fake store, without a second persistence implementation
    pub store: Store,

    /// Exact evidence before the role story, including accepted expense and
    /// original finite funding.
    pub initial_rows: BTreeMap<Key, Record>,

    /// Independent registered asks and observed transactions
    pub referee: Referee,

    /// Actual pre-transaction observer paired with each submitted cohort, for
    /// positive-control and specific corruption negatives
    pub cohorts: Vec<(Referee, Vec<Write>)>,

    /// Exact non-diagnostic boundary trace.
    pub trace: Vec<String>,

    /// Actual captured ordered pages.
    pub pages: u32,

    /// Actual durable role-commit cuts.
    pub restarts: u32,
}

fn config(settings: Settings) -> engine::Config {
    let mut config = walking::config(settings.seed);
    config.owners = Box::new([
        people::InitialOwner { project: 1, identity: people::IdentityKey { forge: 1, user: 7 } },
        people::InitialOwner { project: 1, identity: people::IdentityKey { forge: 1, user: 8 } },
    ]);
    let mut policy = config.authority.policy(1).expect("configured project").clone();
    policy.roles[0].requests = authority::Requests(1 | 4 | if settings.policy { 256 } else { 0 });
    let mut out = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut config.authority, authority::Event::Policy { project: 1, policy }, &mut out);
    assert_eq!(out.pop(), Some(authority::PolicyFact::Changed { project: 1 }));
    config
}

/// Exact immutable child/root capacities for this bounded two-person story
#[must_use]
pub fn limits() -> engine::Limits {
    let mut limits = escalation::limits();
    limits.people.requests = 24;
    limits
}

fn snapshot(settings: Settings) -> Store {
    let story = if settings.base == Base::FinalRole { Story::PassRelease } else { Story::Reject };
    let mut previous = escalation::World::new(escalation::Settings::calm(settings.seed, story));
    if settings.base == Base::Rejected {
        previous.run();
        return previous.store;
    }
    for _ in 0..300 {
        previous.iterate();
        let waiting = previous.store.rows.values().any(|row| {
            matches!(row, Record::Tasks(tasks::Stored::Live(record)) if matches!(record.escalation,
                tasks::Escalation::Waiting { revision: 1, holder: tasks::EscalationHolder::Person(_) }
                    if settings.base == Base::Requester)
                || matches!(record.escalation,
                    tasks::Escalation::Waiting { revision: 2, holder: tasks::EscalationHolder::Role { .. } }
                    if settings.base == Base::FinalRole))
        });
        let acknowledged = previous.trace.iter().any(|entry| entry.starts_with("output Deliver(Acknowledge {"));
        if waiting && acknowledged && previous.store.pending.is_empty() {
            return previous.store;
        }
    }
    panic!("actual prior held-chat process never reached {:?}", settings.base);
}

impl World {
    /// Restore genuine held-chat rows and saved sessions through actual root
    /// startup pages; no direct child inputs are used
    #[must_use]
    pub fn new(settings: Settings) -> World {
        let mut store = snapshot(settings);
        let mut people = [0; 2];
        let mut sessions = [0; 2];
        let mut task = 0;
        for row in store.rows.values() {
            match row {
                Record::People(people::Stored::Person { number, identity }) => {
                    if identity.key.forge == 1 && (7..=8).contains(&identity.key.user) {
                        people[usize::try_from(identity.key.user - 7).expect("two actual identities")] = *number;
                    }
                }
                Record::Tasks(tasks::Stored::Live(record)) => task = record.number,
                Record::People(
                    people::Stored::SignIn { .. }
                    | people::Stored::ReadPosition { .. }
                    | people::Stored::Roles { .. }
                    | people::Stored::Answer { .. },
                )
                | Record::Tasks(tasks::Stored::Ended(_) | tasks::Stored::Ledger(_))
                | Record::Deployment(_)
                | Record::Turn(_)
                | Record::RunProof(_)
                | Record::Terminal(_)
                | Record::EscalationDecision(_) => {}
            }
        }
        for row in store.rows.values() {
            if let Record::People(people::Stored::SignIn { number, person, .. }) = row {
                for index in 0..2 {
                    if *person == people[index] {
                        sessions[index] = *number;
                    }
                }
            }
        }
        assert!(task != 0 && people.iter().all(|person| *person != 0) && sessions.iter().all(|session| *session != 0));
        if settings.exhausted {
            let Some(Record::Tasks(tasks::Stored::Live(record))) =
                store.rows.get_mut(&Key::Tasks(tasks::Key::Live(task)))
            else {
                panic!("actual durable held task");
            };
            let tasks::Escalation::Waiting { revision, .. } = &mut record.escalation else {
                panic!("revision exhaustion belongs only to Waiting");
            };
            *revision = u64::MAX;
        }
        let initial_rows = store.rows.clone();
        let limits = limits();
        let mut world = World {
            domain: engine::Domain::new(config(settings), &limits),
            environment: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(engine::max_out(&limits)),
            events: VecDeque::new(),
            inputs: BTreeMap::new(),
            iteration: 0,
            commit_wait: 0,
            frozen_cut: false,
            recovering: false,
            next_right: 1000,
            people,
            sessions,
            task,
            referee: Referee::new(initial_rows.clone()),
            initial_rows,
            cohorts: Vec::new(),
            trace: Vec::new(),
            pages: 0,
            restarts: 0,
            store,
            settings,
        };
        world.queue(engine::Event::Start, 0);
        world.bootstrap();
        world
    }

    fn queue(&mut self, event: engine::Event, delay: u32) {
        self.events.push_back((self.iteration + delay + 1, event));
    }

    fn bootstrap(&mut self) {
        for _ in 0..300 {
            if self.domain.ready() && self.settled() {
                return;
            }
            self.iterate();
        }
        panic!("paged role root did not restore: {:?}", self.trace);
    }

    fn dispatch(&mut self, to: Token, input: &Input) {
        self.queue(
            engine::Event::Ask {
                reply_to: ReplyTo::new(to),
                sign_in: self.sessions[input.owner],
                key: input.key,
                ask: input.ask.clone(),
            },
            0,
        );
    }

    /// Register one outside keyed ask and its exact expected wrapper. `reroute`
    /// names the scenario's independently expected requester-to-final-role move;
    /// unchanged/final/rejected rows are expected to emit no task writes
    /// (domain/people.md, section 5.1).
    pub fn ask(&mut self, owner: usize, key: u8, ask: people::Ask, reply: people::Reply, reroute: bool) {
        let replacement = if matches!(reply, people::Reply::Outcome(people::Outcome::RolesSet { .. })) {
            let people::Ask::SetRoles { holdings, .. } = &ask else {
                panic!("role success requires SetRoles");
            };
            let mut changed = BTreeMap::new();
            if reroute {
                let mut record = self.task_record().clone();
                let tasks::Escalation::Waiting { revision, holder: tasks::EscalationHolder::Person(_) } =
                    record.escalation
                else {
                    panic!("scripted reroute starts at actual requester Waiting");
                };
                record.escalation = tasks::Escalation::Waiting {
                    revision: revision.checked_add(1).expect("successful script is not exhausted"),
                    holder: tasks::EscalationHolder::Role { project: 1, role: 0 },
                };
                changed.insert(record.number, record);
            }
            Some(Replacement { holdings: holdings.clone(), tasks: changed })
        } else {
            None
        };
        let to = Token::new(self.next_right);
        self.next_right += 1;
        let input = Input { owner, key: [key; 16], ask: ask.clone() };
        self.referee.ask(to, self.people[owner], input.key, ask, reply, replacement);
        self.inputs.insert(to, input.clone());
        self.dispatch(to, &input);
    }

    /// Current task from the fake store's observable durable evidence, never the
    /// child's private state.
    #[must_use]
    pub fn task_record(&self) -> &tasks::TaskRecord {
        let Some(Record::Tasks(tasks::Stored::Live(record))) =
            self.store.rows.get(&Key::Tasks(tasks::Key::Live(self.task)))
        else {
            panic!("role story retains one actual held task");
        };
        record.as_ref()
    }

    /// Complete all registered outside replies and actual pending store IO
    /// within a bounded number of iterations.
    pub fn run(&mut self) {
        for _ in 0..500 {
            if self.settled() {
                return;
            }
            self.iterate();
        }
        panic!("role story did not settle: inputs {:?}; trace {:?}", self.inputs, self.trace);
    }

    /// One real root iteration, with later fake-store terminals and optional
    /// fact draining.
    pub fn iterate(&mut self) {
        self.iteration += 1;
        let nanos = u64::from(self.iteration) * 1_000_000 + if self.settings.expired { 61_000_000_000 } else { 0 };
        self.environment.now = Time::from_nanos(nanos);
        self.environment.wall = Wall::from_nanos(nanos);
        if !self.frozen_cut {
            engine::resume(&mut self.domain, &self.environment, &mut self.out);
            self.observe();
            if !self.frozen_cut && self.events.front().is_some_and(|(due, _)| *due <= self.iteration) {
                let (_, event) = self.events.pop_front().expect("one due outside event");
                self.trace.push(format!("input {event:?}"));
                engine::step(&mut self.domain, &self.environment, event, &mut self.out);
                self.observe();
            }
            if !self.frozen_cut {
                engine::fire(&mut self.domain, &self.environment, &mut self.out);
                self.observe();
                if self.settings.facts {
                    self.domain.drain_facts();
                }
                self.domain.reclaim();
            }
        }
        self.apply_store();
        if self.recovering && self.domain.ready() {
            self.recovering = false;
            let pending: Vec<_> = self.inputs.iter().map(|(to, input)| (*to, input.clone())).collect();
            for (to, input) in pending {
                self.dispatch(to, &input);
            }
        }
    }

    fn apply_store(&mut self) {
        if self.store.pending.is_empty() {
            return;
        }
        if self.commit_wait != 0 {
            self.commit_wait -= 1;
            return;
        }
        let roles = self.store.pending.front().expect("actual pending commit").1.iter().any(|write| {
            matches!(
                write,
                Write::Save(Record::People(people::Stored::Answer { outcome: people::Outcome::RolesSet { .. }, .. }))
            )
        });
        let number = self.store.apply();
        self.trace.push(format!("applied {number}"));
        self.commit_wait = self.settings.commit_delay;
        if self.settings.cut && self.restarts == 0 && roles {
            assert!(self.store.pending.is_empty(), "cut follows one complete role cohort");
            self.domain = engine::Domain::new(config(self.settings), &self.environment.limits);
            self.out = Queue::with_capacity(engine::max_out(&self.environment.limits));
            self.events.clear();
            self.frozen_cut = false;
            self.recovering = true;
            self.restarts += 1;
            self.trace.push("restart after atomic role application".into());
            self.queue(engine::Event::Start, 0);
        } else {
            self.queue(engine::Event::Committed { number }, 0);
        }
    }

    fn observe(&mut self) {
        assert!(self.out.len() <= engine::max_out(&self.environment.limits));
        while let Some(request) = self.out.pop() {
            self.trace.push(format!("output {request:?}"));
            match request {
                engine::Request::Commit { number, writes } => {
                    self.cohorts.push((self.referee.clone(), writes.to_vec()));
                    self.referee.commit(&writes).expect("independent role transaction observer");
                    let roles = writes.iter().any(|write| {
                        matches!(
                            write,
                            Write::Save(Record::People(people::Stored::Answer {
                                outcome: people::Outcome::RolesSet { .. },
                                ..
                            }))
                        )
                    });
                    if self.store.pending.is_empty() {
                        self.commit_wait = self.settings.commit_delay;
                    }
                    self.store.pending.push_back((number, writes));
                    if self.settings.cut && self.restarts == 0 && roles {
                        self.frozen_cut = true;
                        return;
                    }
                }
                engine::Request::Load { owner, range, after, most, .. } => {
                    self.pages += 1;
                    let (rows, next) = self.store.page(range, after, most);
                    self.queue(engine::Event::Loaded { owner, rows, next }, self.settings.page_delay);
                }
                engine::Request::Deliver(Delivery::WebReply { to, reply, .. }) => {
                    let token = to.into_token();
                    assert_eq!(
                        self.referee.replied(&self.store.rows, token, reply),
                        Ok(()),
                        "exact durable role terminal: {token:?} {reply:?}"
                    );
                    self.inputs.remove(&token).expect("one pending outside reply right");
                }
                engine::Request::Deliver(delivery) => panic!("unexpected role-world delivery {delivery:?}"),
                engine::Request::Account(accounts::Request::Refresh { account, generation }) => {
                    self.queue(engine::Event::Refreshed { account, generation, valid: Duration::from_secs(60) }, 0);
                }
                engine::Request::Account(
                    accounts::Request::Keep { .. }
                    | accounts::Request::Cancel { .. }
                    | accounts::Request::Granted { .. }
                    | accounts::Request::Availability { .. }
                    | accounts::Request::Refused { .. }
                    | accounts::Request::Closed { .. },
                ) => {}
                engine::Request::AnswerBusy { .. } | engine::Request::TurnBusy { .. } => {
                    panic!("held role world offers no worker answers")
                }
                engine::Request::Stop => panic!("role root stopped: {:?}", self.trace),
            }
        }
    }

    fn settled(&self) -> bool {
        self.referee.done()
            && self.inputs.is_empty()
            && self.events.is_empty()
            && self.store.pending.is_empty()
            && self.out.is_empty()
            && self.domain.quiescent()
    }
}

/// Replay complete frozen real-root/store/script state after every iteration.
/// The supplied outside operation is retained in both worlds before stepping
/// (testing-strategy.md, section 6).
#[must_use]
pub fn reroute_replayed(settings: Settings, winner: usize) -> World {
    let mut first = World::new(settings);
    let mut replay = World::new(settings);
    for world in [&mut first, &mut replay] {
        let ask = people::Ask::SetRoles {
            project: 1,
            holdings: Box::new([people::Holding { person: world.people[winner], role: people::Role::Owner }]),
        };
        world.ask(
            winner,
            70,
            ask,
            people::Reply::Outcome(people::Outcome::RolesSet { project: 1 }),
            winner == 1 && settings.base == Base::Requester,
        );
    }
    for _ in 0..500 {
        if first.settled() {
            assert!(replay.settled(), "replay settles at identical cut");
            return first;
        }
        first.iterate();
        replay.iterate();
        assert_eq!(format!("{first:?}"), format!("{replay:?}"), "complete frozen role world: {settings:?}");
    }
    panic!("role replay did not settle: {:?}", first.trace);
}
