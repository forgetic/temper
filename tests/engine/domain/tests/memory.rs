//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the engine driven at random through its boundary,
//! by a forge that answers at once and keeps what the engine wrote (its
//! records and outcomes, decoded back), workers that dial in, answer with
//! outcomes as large as the limits allow, relay calls and report, people
//! who open sessions and ask everything they may, and a store; its peak
//! measured at every entry point, as the loop calls them. The driver keeps
//! little of its own, in containers it bounds, and what it keeps is counted
//! with the engine's: the check errs on the side of the bound.

use std::mem::{size_of, size_of_val};
use temper_engine_domain::forge::api::{self, Answer, Body, Comment, Error, Kind, Mark, Op, State, Summary};
use temper_engine_domain::notes::{Author, Change, Page, Recall, Scope, Scopes};
use temper_engine_domain::plan::{self, AgentSpec, ChangeSpec, Repository, Review, Step, Work as Primitive};
use temper_engine_domain::views::Kind as Report;
use temper_engine_domain::{
    Ask, Call, Decoded, Domain, Event, Failure, Hello, Hosted, Item, Landed, Limits, Outcome, Payload, Request, Stored,
    Watched, Work, fire, fleet, max_out, resume, step, worst_case,
};

use skein_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token, Wall};
use temper_engine_domain::forge::Position;
use temper_engine_domain_world::codec;
use temper_engine_domain_world::deployment::{self, BUDGET, LIMITS};
use temper_engine_forge_world::translate;
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

/// Few items, so that every bound is met often.
const SMALL: Limits = Limits {
    work: temper_engine_domain::work::Limits { items: 6, ..LIMITS.work },
    forge: temper_engine_domain::forge::Limits { items: 6, ..LIMITS.forge },
    ..LIMITS
};

/// The most comments the driver's forge keeps on an item: older ones are
/// forgotten, as far as the engine reads them.
const COMMENTS: usize = 6;

fn bytes(len: u32, fill: u8) -> Box<[u8]> {
    vec![fill; usize::try_from(len).expect("fits")].into_boxed_slice()
}

/// A comment the driver's forge keeps: its id, its mark, and the body the
/// engine's payload was written as, if it carried one.
type Kept = (u64, Mark, Option<Box<[u8]>>);

/// An item of the driver's forge.
struct Issue {
    number: u64,
    open: bool,
    labels: Box<[Box<[u8]>]>,
    /// Each comment's id, mark, and the body the engine's payload was
    /// written as, if it carried one.
    comments: Vec<Kept>,
}

/// What the driver has in hand: the forge's items, the workers' channels and
/// the attempts assigned, people's asks, and watchers.
struct Driver {
    rng: Rng,
    limits: Limits,
    issues: Vec<Issue>,
    comments: u64,
    channels: Vec<Token>,
    assigned: Vec<(Token, Item, u64)>,
    calls: u64,
    assignments: u32,
    asks: u64,
    watchers: Vec<Token>,
    pending: Vec<Event>,
    /// The bytes the driver's forge holds of its own: counted with the
    /// engine's by the allocator, and taken off the measures.
    held: u64,
}

/// The engine, measured at every entry point against its worst case.
struct Measured {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    /// What a new domain holds beyond its worst case.
    excess: u64,
    /// The most held at once between entry points, and at any moment.
    fullest: u64,
    peak: u64,
}

impl Measured {
    /// The engine measured against its worst case and `margin` more.
    fn new(limits: &Limits, margin: u64) -> Measured {
        Measured::configured(limits, margin, normal_config)
    }

    fn configured(limits: &Limits, margin: u64, config: fn(&Limits) -> temper_engine_domain::Config) -> Measured {
        let bound = worst_case(limits).expect("the test limits fit");
        // The shell owns its output queue; the domain owns the configuration.
        let out = Queue::with_capacity(max_out(limits));
        let meter = Meter::new();
        meter.start();
        let config = config(limits);
        let domain = Domain::new(config, limits, 7, Time::ZERO);
        let measured = meter.end();
        meter.check(measured, bound, "configuration and constructor");
        let excess = meter.held().saturating_sub(bound);
        let bound = bound + margin;
        Measured {
            domain,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits },
            out,
            meter,
            bound,
            excess,
            fullest: 0,
            peak: 0,
        }
    }

    /// Steps `event`, then fires and resumes what is due, each measured.
    fn step(&mut self, event: Event, driver: &mut Driver) {
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain(driver);
        self.turn(driver);
    }

    fn turn(&mut self, driver: &mut Driver) {
        for _ in 0..256 {
            if self.domain.is_ready() {
                self.meter.start();
                resume(&mut self.domain, &self.env, &mut self.out);
            } else if self.domain.is_due(self.env.now) {
                self.meter.start();
                fire(&mut self.domain, &self.env, &mut self.out);
            } else {
                return;
            }
            self.drain(driver);
        }
    }

    fn drain(&mut self, driver: &mut Driver) {
        let measured = self.meter.end();
        let mut requests = Vec::with_capacity(usize::try_from(self.out.len()).expect("fits"));
        while let Some(request) = self.out.pop() {
            requests.push(request);
        }
        for request in requests {
            driver.answer(request);
        }
        self.peak = self.peak.max(measured.peak.saturating_sub(driver.held));
        self.meter.check(measured, self.bound.saturating_add(driver.held), "an entry point");
        while self.domain.pop_fact().is_some() {}
        self.domain.reclaim();
        if driver.pending.is_empty() {
            self.fullest = self.fullest.max(self.meter.held().saturating_sub(driver.held));
        }
    }
}

impl Driver {
    fn new(limits: &Limits, seed: u64) -> Driver {
        Driver {
            rng: Rng::new(seed),
            limits: *limits,
            issues: Vec::with_capacity(64),
            comments: 0,
            channels: Vec::with_capacity(8),
            assigned: Vec::with_capacity(64),
            calls: 0,
            assignments: 0,
            asks: 0,
            watchers: Vec::with_capacity(8),
            pending: Vec::with_capacity(64),
            held: 0,
        }
    }

    fn issue(&mut self, number: u64) -> Option<&mut Issue> {
        self.issues.iter_mut().find(|issue| issue.number == number)
    }

    /// What the engine asked of its neighbours: the forge and the store
    /// answer at once; the rest is noted.
    fn answer(&mut self, request: Request) {
        match request {
            Request::Forge { call, repository: _, op, payload } => {
                let (result, decoded) = self.forge(op, payload);
                self.pending.push(Event::Answered { call, result, decoded });
            }
            Request::Store { owner, op } => {
                let stored = match op {
                    temper_engine_domain::Store::Get { .. } => {
                        Stored::Got(if self.rng.chance(500) { Some(bytes(64, b's')) } else { None })
                    }
                    temper_engine_domain::Store::Put { .. }
                    | temper_engine_domain::Store::Drop { .. }
                    | temper_engine_domain::Store::Append { .. }
                    | temper_engine_domain::Store::Expire { .. } => {
                        if self.rng.chance(100) {
                            Stored::Failed
                        } else {
                            Stored::Done
                        }
                    }
                };
                self.pending.push(Event::Stored { owner, stored });
            }
            Request::Assign { channel, assignment } => {
                self.assignments += 1;
                assert!(assignment.charter.models.len() <= usize::try_from(self.limits.models_bytes).expect("fits"));
                if self.assigned.len() < 64 {
                    self.assigned.push((channel, assignment.item, assignment.attempt));
                }
            }
            Request::Deliver { watcher, .. } => self.pending.push(Event::Delivered { watcher, done: true }),
            Request::Reply { reply: temper_engine_domain::Reply::Watching { watcher }, .. } => {
                if self.watchers.len() < 8 {
                    self.watchers.push(watcher);
                }
            }
            Request::Refuse { channel } => self.channels.retain(|open| *open != channel),
            Request::Account { .. }
            | Request::Grant { .. }
            | Request::Inbound { .. }
            | Request::Cancel { .. }
            | Request::Relayed { .. }
            | Request::Acknowledge { .. }
            | Request::Reply { .. }
            | Request::Ended { .. } => {}
        }
    }

    /// The forge's answer to `op`, and what of the engine's it holds.
    #[expect(clippy::too_many_lines, reason = "one arm per op")]
    fn forge(&mut self, op: Op, payload: Option<Payload>) -> (Result<Answer, Error>, Box<[Decoded]>) {
        if self.rng.chance(50) {
            return (Err(Error::Unavailable), Box::new([]));
        }
        let page = usize::try_from(self.limits.forge.page).expect("fits");
        let answer = match op {
            Op::Items { .. } => {
                let items = self.issues.iter().filter(|issue| issue.open).take(page).map(summary).collect();
                Ok(Answer::Items { items, more: false, now: Time::ZERO })
            }
            Op::Item { number, after } => {
                let Some(issue) = self.issue(number) else { return (Err(Error::Missing), Box::new([])) };
                let mut decoded = Vec::new();
                let mut comments = Vec::new();
                for (id, mark, found) in &issue.comments {
                    if *id <= after {
                        continue;
                    }
                    if let Some(found) = found.as_deref().and_then(|body| codec::comment(*id, body)) {
                        decoded.push(found);
                    }
                    comments.push(comment(*id, mark));
                }
                let item = summary(issue);
                return (Ok(Answer::Item { item, comments: comments.into(), more: false }), decoded.into());
            }
            Op::Comment { number, id } => {
                let Some(issue) = self.issue(number) else { return (Err(Error::Missing), Box::new([])) };
                let Some((id, mark, found)) = issue.comments.iter().find(|(at, _, _)| *at == id) else {
                    return (Err(Error::Missing), Box::new([]));
                };
                let decoded = found.as_deref().and_then(|body| codec::comment(*id, body)).into_iter().collect();
                return (Ok(Answer::Comment(comment(*id, mark))), decoded);
            }
            Op::Permission { .. } => Ok(Answer::Permission(api::Permission::Admin)),
            Op::OpenPull { .. } => Ok(Answer::Created(100 + u64::try_from(self.issues.len()).expect("few"))),
            Op::CreateIssue { labels, .. } => {
                let number = u64::try_from(self.issues.len()).expect("few") + 1;
                if self.issues.len() < 48 {
                    let comments = Vec::with_capacity(COMMENTS + 1);
                    self.held += labels_heap(&labels) + capacity_heap(&comments);
                    self.issues.push(Issue { number, open: true, labels, comments });
                }
                Ok(Answer::Created(number))
            }
            Op::Post { number, key, person, body } => {
                self.comments += 1;
                let id = self.comments;
                let mark = match body {
                    Body::Record { position, nonce, .. } => Mark::Record { position, nonce },
                    Body::Text(_) | Body::Payload(_) => match key {
                        Some(key) => Mark::Key { key, person },
                        None => Mark::None,
                    },
                };
                let found = written(payload);
                let size = mark_heap(&mark) + found.as_ref().map_or(0, |body| len(body));
                let mut freed = 0;
                if let Some(issue) = self.issue(number) {
                    issue.comments.push((id, mark, found));
                    if issue.comments.len() > COMMENTS {
                        let (_, mark, body) = issue.comments.remove(1);
                        freed = mark_heap(&mark) + body.as_ref().map_or(0, |body| len(body));
                    }
                    self.held = self.held + size - freed;
                }
                Ok(Answer::Commented { id, revision: 1 })
            }
            Op::EditComment { number, id, body } => {
                let found = written(payload);
                let size = found.as_ref().map_or(0, |body| len(body));
                let mut freed = None;
                if let Some(issue) = self.issue(number)
                    && let Some(entry) = issue.comments.iter_mut().find(|(at, _, _)| *at == id)
                {
                    if let Body::Record { position, nonce, .. } = body {
                        entry.1 = Mark::Record { position, nonce };
                    }
                    freed = Some(entry.2.as_ref().map_or(0, |body| len(body)));
                    entry.2 = found;
                }
                if let Some(freed) = freed {
                    self.held = self.held + size - freed;
                }
                Ok(Answer::Edited { revision: 2 })
            }
            Op::Close { number } => {
                if let Some(issue) = self.issue(number) {
                    issue.open = false;
                }
                Ok(Answer::Done)
            }
            Op::Pages { .. } => Ok(Answer::Pages { pages: Box::new([]), next: None }),
            Op::Merge { head, .. } => Ok(Answer::Merged(head)),
            Op::Review { .. } => Ok(Answer::Reviewed(1)),
            Op::PutPage { .. } => Ok(Answer::Revision(1)),
            Op::Pull { .. }
            | Op::PullFor { .. }
            | Op::Reviews { .. }
            | Op::Statuses { .. }
            | Op::Remarks { .. }
            | Op::Branch { .. }
            | Op::Page { .. } => Err(Error::Missing),
            Op::AddLabels { .. }
            | Op::RemoveLabels { .. }
            | Op::SetReviewers { .. }
            | Op::SetDependencies { .. }
            | Op::Reopen { .. }
            | Op::DeleteBranch { .. }
            | Op::DeletePage { .. } => Ok(Answer::Done),
        };
        (answer, Box::new([]))
    }

    /// Something a neighbour does, drawn at random.
    fn act(&mut self, domain: &Domain) -> Option<Event> {
        let limits = self.limits;
        let at = self.index(self.issues.len());
        let item = self.issues.get(at).map(|issue| Item { repository: 0, number: issue.number });
        let text = bytes(limits.text_bytes, b't');
        match self.rng.below(12) {
            0 => {
                self.asks += 1;
                let key = format!("k{}", self.asks).into_bytes().into_boxed_slice();
                let ask = Ask::Open { repository: 0, key, title: bytes(16, b'h'), message: text };
                Some(self.ask(ask))
            }
            1 => {
                let item = item?;
                self.asks += 1;
                let key = format!("k{}", self.asks).into_bytes().into_boxed_slice();
                Some(self.ask(Ask::Message { item, key, message: text }))
            }
            2 => {
                let item = item?;
                let ask = match self.rng.below(6) {
                    0 => Ask::Accept { item },
                    1 => Ask::Reject { item },
                    2 => Ask::Stop { item },
                    3 => Ask::Release { item },
                    4 => Ask::Watch { subject: Watched::Run { item } },
                    _ => Ask::Watch { subject: Watched::Item { item } },
                };
                Some(self.ask(ask))
            }
            3 => {
                if self.channels.len() >= 4 {
                    let channel = self.channels.remove(0);
                    return Some(Event::Lost { channel });
                }
                self.calls += 1;
                let channel = Token::new(1_000 + self.calls);
                self.channels.push(channel);
                let hosting: Vec<Hosted> = self
                    .assigned
                    .iter()
                    .take(4)
                    .map(|&(_, item, attempt)| Hosted { item, attempt, phase: fleet::Phase::Active })
                    .collect();
                Some(Event::Hello {
                    channel,
                    hello: Hello { slots: 4, workstreams: Box::new([]), hosting: hosting.into() },
                })
            }
            4..=6 => {
                if self.assigned.is_empty() {
                    return None;
                }
                let at = self.index(self.assigned.len());
                let (channel, item, attempt) = self.assigned.remove(at);
                let answer = self.answered(item);
                Some(Event::Answer { channel, item, attempt, answer })
            }
            7 | 8 => {
                let at = self.index(self.assigned.len());
                let &(channel, item, attempt) = self.assigned.get(at)?;
                self.calls += 1;
                let call = Token::new(self.calls);
                let body = self.call(item);
                Some(Event::Relay { channel, item, attempt, call, body })
            }
            9 => {
                let at = self.index(self.assigned.len());
                let &(channel, item, attempt) = self.assigned.get(at)?;
                let content = bytes(limits.views.report_bytes.saturating_mul(2), b'r');
                Some(Event::Told { channel, item, attempt, kind: Report::Text, content })
            }
            10 => {
                let at = self.index(self.watchers.len());
                let watcher = *self.watchers.get(at)?;
                self.watchers.retain(|open| *open != watcher);
                Some(Event::Unwatch { watcher })
            }
            _ => {
                let at = self.index(self.issues.len());
                let number = self.issues.get(at)?.number;
                let _ = domain;
                Some(Event::Hint { repository: 0, item: Some(number), commit: None, branch: None })
            }
        }
    }

    fn ask(&mut self, ask: Ask) -> Event {
        self.calls += 1;
        Event::Ask { reply_to: ReplyTo::new(Token::new(self.calls)), person: 10, ask }
    }

    fn index(&mut self, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        usize::try_from(self.rng.below(u64::try_from(len).expect("few"))).expect("an index")
    }

    /// A run's answer, its outcome as large as the limits allow.
    fn answered(&mut self, item: Item) -> temper_engine_domain::Answer {
        let limits = self.limits;
        let work = Work { landed: Box::new([Landed { repository: 0, commit: [1; 32] }]) };
        let text = bytes(limits.text_bytes, b'o');
        let outcome = match self.rng.below(9) {
            0 => Outcome::Reply { text },
            1 => Outcome::Finished { text },
            2 => Outcome::Report { text },
            3 => Outcome::Change { message: text },
            4 => Outcome::Tasks { tasks: steps(&limits, limits.plan.tasks, b'k'), text },
            5 => Outcome::Steps { steps: steps(&limits, 2, b'g'), text },
            6 => Outcome::Plan {
                plan: plan::Plan {
                    steps: steps(&limits, limits.plan.steps, b'p'),
                    envelope: envelope(),
                    budget: 10_000,
                },
                text,
            },
            7 => {
                return match self.rng.below(3) {
                    0 => temper_engine_domain::Answer::Parked {
                        snapshot: Some(bytes(limits.views.snapshot_bytes, b's')),
                        work,
                    },
                    1 => temper_engine_domain::Answer::Failed { failure: Failure::Run, work },
                    _ => temper_engine_domain::Answer::Busy,
                };
            }
            _ => Outcome::Escalation { text },
        };
        let _ = item;
        temper_engine_domain::Answer::Ended { outcome, work }
    }

    /// A run's call: a recall, a note, a comment, an escalation.
    fn call(&mut self, item: Item) -> Call {
        let limits = self.limits;
        let name = bytes(limits.notes.name_bytes, b'n');
        match self.rng.below(4) {
            0 => Call::Recall(if self.rng.chance(500) {
                Recall::Name { scope: Scope::Repository(0), name }
            } else {
                Recall::Search { scopes: Scopes { repository: 0, goal: None }, query: bytes(8, b'q'), most: 4 }
            }),
            1 => {
                let page = Page {
                    description: bytes(limits.notes.description_bytes, b'd'),
                    author: Author::Run { repository: item.repository, number: item.number },
                    references: Box::new([]),
                    body: bytes(limits.notes.body_bytes, b'b'),
                };
                Call::Note { scope: Scope::Repository(0), name, change: Change::New(page) }
            }
            2 => Call::Comment { text: bytes(limits.text_bytes, b'c') },
            _ => Call::Escalate { text: bytes(limits.text_bytes, b'e') },
        }
    }
}

fn summary(issue: &Issue) -> Summary {
    Summary {
        number: issue.number,
        kind: Kind::Issue,
        state: if issue.open { State::Open } else { State::Closed },
        author: 10,
        key: None,
        labels: issue.labels.clone(),
        title: Box::new([]),
        body: Box::new([]),
        updated: Time::ZERO,
    }
}

fn comment(id: u64, mark: &Mark) -> Comment {
    let mark = match mark {
        Mark::None => Mark::None,
        Mark::Key { key, person } => Mark::Key { key: key.clone(), person: *person },
        Mark::Record { position, nonce } => Mark::Record { position: *position, nonce: *nonce },
        Mark::Mangled => Mark::Mangled,
    };
    Comment { id, author: deployment::ENGINE, created: Time::ZERO, revision: 1, mark, body: Box::new([]) }
}

/// The body the protocol layer writes a payload as, which it decodes back
/// when the comment is read.
fn written(payload: Option<Payload>) -> Option<Box<[u8]>> {
    let body = match payload? {
        Payload::Record(record) => translate::recorded(Position::START, 0, &codec::record_block(&record)),
        Payload::Outcome(posted) => translate::keyed(b"k", &codec::posted_block(&posted)),
        Payload::Page(_) => return None,
    };
    Some(body.into_boxed_slice())
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("fits")
}

fn labels_heap(labels: &[Box<[u8]>]) -> u64 {
    let boxes = u64::try_from(size_of_val(labels)).expect("fits");
    boxes + labels.iter().map(|label| len(label)).sum::<u64>()
}

fn capacity_heap<T>(vec: &Vec<T>) -> u64 {
    u64::try_from(vec.capacity() * size_of::<T>()).expect("fits")
}

fn mark_heap(mark: &Mark) -> u64 {
    match mark {
        Mark::Key { key, .. } => len(key),
        Mark::None | Mark::Record { .. } | Mark::Mangled => 0,
    }
}

/// `count` steps of the plan's largest, alternating agents and changes, each
/// after the one before.
fn steps(limits: &Limits, count: u32, fill: u8) -> Box<[Step]> {
    let plan = &limits.plan;
    (0..count)
        .map(|at| {
            let mut name = bytes(plan.name_bytes, fill);
            name[0] = b'a' + u8::try_from(at % 26).expect("a letter");
            let charter = plan::Charter {
                instructions: bytes(plan.instruction_bytes, b'i'),
                template: None,
                grants: plan::Grants { modify: true, shell: true, forge: true, subagents: false, note: true },
                budget: BUDGET,
            };
            let work = if at % 2 == 0 {
                Primitive::Agent(AgentSpec { charter, grows: false })
            } else {
                Primitive::Change(ChangeSpec {
                    base: deployment::MAIN.into(),
                    produce: charter,
                    checks: true,
                    review: Review::Person,
                })
            };
            Step { name, repository: Repository(0), work, after: Box::new([]), gates: Box::new([]) }
        })
        .collect()
}

fn envelope() -> plan::Envelope {
    plan::Envelope {
        agents: 4,
        changes: 4,
        waits: 1,
        sessions: 0,
        repositories: Box::new([Repository(0)]),
        into: Box::new([plan::Target { repository: Repository(0), base: deployment::MAIN.into() }]),
    }
}

/// Drives the engine at random for `rounds`, and says the most it held at
/// once between entry points.
fn run(limits: &Limits, seed: u64, rounds: u32, margin: u64) -> (Measured, Driver) {
    run_configured(limits, seed, rounds, margin, normal_config)
}

fn run_configured(
    limits: &Limits,
    seed: u64,
    rounds: u32,
    margin: u64,
    config: fn(&Limits) -> temper_engine_domain::Config,
) -> (Measured, Driver) {
    // The driver's containers first: what they hold later is counted with
    // the engine's, their room is not.
    let mut driver = Driver::new(limits, seed);
    let mut domain = Measured::configured(limits, margin, config);
    for round in 0..rounds {
        domain.env.now = Time::ZERO.saturating_add(Duration::from_secs(u64::from(round)));
        domain.turn(&mut driver);
        for _ in 0..64 {
            if driver.pending.is_empty() {
                break;
            }
            let event = driver.pending.remove(0);
            domain.step(event, &mut driver);
        }
        if let Some(event) = driver.act(&domain.domain) {
            domain.step(event, &mut driver);
        }
    }
    (domain, driver)
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    for seed in 0..10 {
        let (domain, driver) = run(&SMALL, seed, 600, 0);
        assert!(
            domain.fullest <= domain.bound,
            "seed {seed}: {} held of a worst case of {}",
            domain.fullest,
            domain.bound
        );
        assert!(!driver.issues.is_empty(), "seed {seed}: the engine made items");
    }
}

#[test]
fn the_engine_full_to_its_limits_stays_within_its_worst_case() {
    let mut most = 0;
    let mut bound = 0;
    for seed in 0..4 {
        let (domain, _) = run(&SMALL, seed, 1_500, 0);
        most = most.max(domain.fullest);
        bound = domain.bound;
        assert!(domain.domain.items() > 0, "seed {seed}: items were held");
    }
    // At its fullest, the engine holds a fair share of the bound: it is not
    // a bound by orders of magnitude.
    assert!(most.saturating_mul(2) > bound, "{most} held of a worst case of {bound}");
}

/// A new engine holds no more than its worst case says.
#[test]
fn a_new_engine_holds_no_more_than_its_worst_case() {
    let domain = Measured::new(&SMALL, 0);
    assert_eq!(domain.excess, 0, "a new engine holds {} bytes beyond its worst case", domain.excess);
}

fn normal_config(_limits: &Limits) -> temper_engine_domain::Config {
    deployment::config()
}

/// Fill every retained configuration array and byte limit. Keep the
/// tracking labels used by the driver's forge during the run.
fn full_config(limits: &Limits) -> temper_engine_domain::Config {
    let mut config = deployment::config();
    config.models = Box::new([temper_engine_domain::Model {
        endpoint: 0,
        model: bytes(
            limits.models_bytes.saturating_sub(
                u32::try_from(core::mem::size_of::<temper_engine_domain::Model>()).expect("model size fits"),
            ),
            b'm',
        ),
        max_tokens: 1024,
    }]);
    config.plan.templates = (0..limits.plan.templates)
        .map(|_| plan::Template {
            name: bytes(limits.plan.name_bytes, b't'),
            guidance: bytes(limits.plan.instruction_bytes, b'g'),
        })
        .collect();
    for repo in &mut config.plan.repositories {
        repo.bases = (0..limits.plan.bases)
            .map(|index| if index == 0 { deployment::MAIN.into() } else { bytes(limits.plan.name_bytes, b'b') })
            .collect();
    }
    config.forge.projected = (0..limits.forge.labels).map(|_| bytes(limits.forge.name_bytes, b'p')).collect();
    config.session.charter.instructions = bytes(limits.plan.instruction_bytes, b'i');
    config.session.charter.template = config.plan.templates.first().map(|template| template.name.clone());
    assert!(temper_engine_domain::accepts(&config, limits));
    config
}

#[test]
fn full_configuration_and_large_model_copies_stay_within_the_bound() {
    let limits = Limits { models_bytes: 65_536, ..SMALL };
    let (domain, driver) = run_configured(&limits, 7, 600, 0, full_config);
    assert_eq!(domain.excess, 0);
    assert!(driver.assignments > 1, "the engine copied its models into multiple assignments");
    assert!(domain.peak > u64::from(limits.models_bytes).saturating_mul(2), "the model copies were measured");
}

fn constructor_config(limits: &Limits) -> temper_engine_domain::Config {
    let mut config = full_config(limits);
    config.forge.tracking = bytes(limits.forge.name_bytes, b'l');
    config.forge.hand_in = bytes(limits.forge.name_bytes, b'h');
    config.branches = bytes(limits.forge.name_bytes, b'b');
    config.saved = bytes(limits.forge.name_bytes, b's');
    for repo in &mut config.plan.repositories {
        for base in &mut repo.bases {
            *base = bytes(limits.plan.name_bytes, b'b');
        }
    }
    assert!(temper_engine_domain::accepts(&config, limits));
    config
}

#[test]
fn a_constructor_counts_every_configuration_array_filled_to_its_limit() {
    let limits = Limits {
        models_bytes: 65_536,
        plan: plan::Limits { templates: 64, bases: 64, ..SMALL.plan },
        forge: temper_engine_domain::forge::Limits { labels: 64, ..SMALL.forge },
        ..SMALL
    };
    let domain = Measured::configured(&limits, 0, constructor_config);
    assert_eq!(domain.excess, 0);
}

#[test]
fn an_oversized_session_is_refused_without_copying_its_bytes() {
    let mut config = deployment::config();
    config.session.charter.instructions = bytes(SMALL.plan.instruction_bytes.saturating_add(1), b'i');
    let meter = Meter::new();
    meter.start();
    let accepted = temper_engine_domain::accepts(&config, &SMALL);
    let measured = meter.end();
    assert!(!accepted);
    assert_eq!(measured.peak, 0, "configuration validation allocated before checking its byte limit");
}
