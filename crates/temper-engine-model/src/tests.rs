//! The top level's step tests: its routing and translations, through its
//! boundary, with the forge, workers and people scripted inline.

#![expect(
    clippy::arithmetic_side_effects,
    reason = "the tests count small numbers, and an overflow traps in a test as anywhere"
)]

use alloc::boxed::Box;

use temper_engine_model_brief::{self as brief, Budgets};
use temper_engine_model_fleet as fleet;
use temper_engine_model_forge::{self as forge, api};
use temper_engine_model_notes as notes;
use temper_engine_model_plan::{self as plan, Budget};
use temper_engine_model_rules::{self as rules, Acts, Permission, Rules};
use temper_engine_model_views::{self as views, Capture, Policy};
use temper_engine_model_work::{self as work, Retries, Retry};
use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::boundary::{Ask, Event, Hello, Item, Refusal, Reply, Request};
use crate::config::Config;
use crate::limits::{Limits, accepts, worst_case};
use crate::model::{Model, fire, max_out, step};
use crate::translate;

const RETRY: Retry = Retry { retries: 2, base: Duration::from_secs(1), max: Duration::from_secs(8) };

const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(600) };

const LIMITS: Limits = Limits {
    work: work::Limits {
        items: 4,
        retries: Retries { transient: RETRY, permanent: RETRY, run: RETRY, agent: RETRY, lost: RETRY, invalid: RETRY },
        undelivered: 2,
        facts: 64,
    },
    plan: plan::Limits {
        steps: 8,
        name_bytes: 16,
        dependencies: 4,
        gates: 2,
        targets: 2,
        instruction_bytes: 64,
        tasks: 3,
        events: 8,
        repairs: 2,
        rebases: 4,
        rejections: 2,
        stall: Duration::from_secs(3600),
        budget: BUDGET,
    },
    rules: rules::Limits { repositories: 2, protected: 2, branch_bytes: 16, grants: 4, reviews: 4, gates: 4, lands: 4 },
    forge: forge::Limits {
        repositories: 2,
        items: 4,
        labels: 3,
        members: 3,
        inbox: 4,
        reviewers: 4,
        reads: 16,
        writes: 16,
        calls: 16,
        page: 4,
        name_bytes: 32,
        title_bytes: 64,
        body_bytes: 256,
        rate: 1000,
        window: Duration::from_secs(60),
        reserve: 0,
        poll: Duration::from_secs(30),
        hinted: Duration::from_secs(2),
        resolution: Duration::from_secs(1),
        slow: Duration::from_secs(600),
        probes: 2,
        backoff: Duration::from_secs(1),
        backoff_max: Duration::from_secs(8),
        attempts: 3,
        lifetime: Duration::ZERO,
        facts: 256,
    },
    fleet: fleet::Limits {
        workers: 2,
        slots: 2,
        workstreams: 2,
        workstream_bytes: 16,
        attempts: 8,
        calls: 4,
        grace: Duration::from_secs(10),
        facts: 64,
    },
    brief: brief::Limits {
        briefs: 4,
        sections: 10,
        items: 4,
        parts: 4,
        read_bytes: 256,
        budgets: Budgets {
            item: 64,
            comments: 64,
            dependencies: 64,
            ci: 64,
            reviews: 64,
            pull: 64,
            attempts: 64,
            plan: 64,
            notes: 64,
            template: 64,
        },
        brief_bytes: 640,
        gather: Duration::from_secs(10),
        facts: 64,
    },
    notes: notes::Limits {
        scopes: 3,
        entries: 4,
        name_bytes: 16,
        description_bytes: 32,
        body_bytes: 64,
        references: 2,
        calls: 3,
        lines: 4,
        recalled: 2,
        facts: 64,
    },
    views: views::Limits {
        runs: 4,
        watchers: 4,
        backlog: 4,
        report_bytes: 64,
        records: 4,
        batch_bytes: 256,
        appends: 2,
        flush: Duration::from_secs(1),
        retention: Duration::from_secs(600),
        sweep: Duration::from_secs(60),
        facts: 64,
    },
    asks: 4,
    text_bytes: 256,
    steps: 16,
    facts: 64,
};

/// The engine's forge user, and a person.
const ENGINE: u64 = 99;
const ALICE: u64 = 1;

const ITEM: Item = Item { repository: 0, number: 7 };

/// A deployment of two repositories, the first protecting `main`.
fn config() -> Config {
    let mut protected = List::with_capacity(LIMITS.rules.protected);
    protected.push(rules::Branch { repository: 0, name: copy_of(b"main") }).unwrap();
    let repo = plan::Repo { bases: Box::new([copy_of(b"main")]) };
    let charter = plan::Charter {
        instructions: copy_of(b"talk"),
        template: None,
        grants: plan::Grants { modify: false, shell: false, forge: true, subagents: false, note: true },
        budget: BUDGET,
    };
    let wake = plan::Wake {
        on: plan::Sources { own: true, related: true, subscribed: false, messages: true },
        every: None,
        batch: plan::Batch { count: 1, age: None },
    };
    Config {
        plan: plan::Config { repositories: Box::new([repo.clone(), repo]), templates: Box::new([]) },
        home: 0,
        forge: forge::Config {
            engine: ENGINE,
            tracking: copy_of(b"temper"),
            hand_in: copy_of(b"temper:hand-in"),
            projected: Box::new([]),
        },
        rules: Rules {
            repositories: 2,
            protected,
            engine: ENGINE,
            reviewer: Permission::Write,
            plan_steps: 8,
            plan_spend: 1000,
            plan_acceptance: Permission::Write,
            repository_notes: None,
            deployment_notes: Some(Permission::Admin),
            run_spend: 1000,
            goal_spend: 10_000,
            deployment_spend: 100_000,
            acts: Acts {
                open: Permission::Write,
                steer: Permission::Write,
                accept: Permission::Write,
                cancel: Permission::Write,
                release: Permission::Write,
                watch: Permission::Read,
            },
        },
        session: plan::SessionSpec { charter, resume: plan::Resume::Default, wake },
        models: copy_of(b"model"),
        policy: Policy {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Shape,
            tools: Capture::Shape,
            usage: Capture::Shape,
        },
        branches: copy_of(b"temper/"),
        saved: copy_of(b"temper-saved/"),
    }
}

const fn env(secs: u64) -> Env<Limits> {
    Env { now: Time::from_nanos(secs.saturating_mul(1_000_000_000)), limits: LIMITS }
}

fn model() -> Model {
    assert!(worst_case(&LIMITS).is_some(), "the limits are bounded");
    Model::new(config(), &LIMITS, 1)
}

/// What one event leads to.
fn stepped(model: &mut Model, env: &Env<Limits>, event: Event) -> List<Request> {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    step(model, env, event, &mut out);
    drained(&mut out)
}

/// What the loop does between events at `env.now`: fire what is due, and go
/// on with the ready lists, until neither has anything left.
fn idle(model: &mut Model, env: &Env<Limits>) -> List<Request> {
    let mut requests = List::with_capacity(256);
    for _ in 0_u32..64 {
        let mut out = Queue::with_capacity(max_out(&LIMITS));
        if model.is_ready() {
            crate::model::resume(model, env, &mut out);
        } else if model.is_due(env.now) {
            fire(model, env, &mut out);
        } else {
            return requests;
        }
        while let Some(request) = out.pop() {
            requests.push(request).unwrap();
        }
    }
    panic!("the loop settles");
}

fn drained(out: &mut Queue<Request>) -> List<Request> {
    let mut requests = List::with_capacity(out.len());
    while let Some(request) = out.pop() {
        requests.push(request).unwrap();
    }
    requests
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the limits are bounded");
    assert!(bound > 0, "the model holds something");
    let more = worst_case(&Limits { steps: 32, ..LIMITS }).expect("more steps are bounded");
    assert!(more > bound, "more steps hold more");
    assert_eq!(worst_case(&Limits { steps: 1, ..LIMITS }), None, "a step bound leaves room for hand-offs");
    let fewer = forge::Limits { items: 2, ..LIMITS.forge };
    assert_eq!(worst_case(&Limits { forge: fewer, ..LIMITS }), None, "the hub and the forge hold one working set");
    let narrow = fleet::Limits { workstream_bytes: 8, ..LIMITS.fleet };
    assert_eq!(worst_case(&Limits { fleet: narrow, ..LIMITS }), None, "a workstream key fits the fleet");
    assert!(accepts(&config(), &LIMITS), "the configuration fits");
    let elsewhere = Config { home: 2, ..config() };
    assert!(!accepts(&elsewhere, &LIMITS), "the home is one of the deployment's repositories");
}

#[test]
fn a_run_token_packs_its_item() {
    let run = translate::run(Item { repository: 3, number: 41 }).unwrap();
    assert_eq!(translate::item(run), Item { repository: 3, number: 41 });
    assert_eq!(translate::run(Item { repository: 1 << 16, number: 1 }), None);
    assert_eq!(translate::run(Item { repository: 0, number: 1 << 48 }), None);
    let workstream = translate::workstream(Item { repository: 1, number: 2 });
    assert_eq!(&workstream[..], &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2][..]);
}

#[test]
fn numbers_and_branches_are_written_in_decimal() {
    assert_eq!(&translate::decimal(0)[..], b"0");
    assert_eq!(&translate::decimal(1207)[..], b"1207");
    assert_eq!(&translate::decimal(u64::MAX)[..], b"18446744073709551615");
    assert_eq!(&translate::branch(b"temper/", ITEM)[..], b"temper/7");
}

#[test]
fn hold_reasons_keep_their_codes() {
    let codes = [
        translate::hold(plan::Hold::Rejected),
        translate::hold(plan::Hold::Repairs),
        translate::hold(plan::Hold::Rebases),
        translate::hold(plan::Hold::PullClosed),
        translate::hold(plan::Hold::Escalated),
        translate::hold(plan::Hold::Stalled),
    ];
    assert_eq!(codes, [1, 2, 3, 4, 5, 6], "a stored code means the same after a restart");
    assert!(!codes.contains(&translate::NO_STEP), "no plan reason is the top level's own");
}

#[test]
fn a_cold_start_lists_live_work() {
    let mut model = model();
    let requests = idle(&mut model, &env(1));
    let mut listed = false;
    for request in &requests {
        if let Request::Forge { op: api::Op::Items { .. }, payload: None, .. } = request {
            listed = true;
        }
    }
    assert!(listed, "the forge sub-model lists live work: {requests:?}");
}

#[test]
fn a_person_asking_of_an_item_not_held_is_refused() {
    let mut model = model();
    let ask = Ask::Stop { item: ITEM };
    let requests =
        stepped(&mut model, &env(1), Event::Ask { reply_to: ReplyTo::new(Token::new(5)), person: ALICE, ask });
    let [Request::Reply { to, reply }] = requests.as_slice() else { panic!("one reply: {requests:?}") };
    assert_eq!(*reply, Reply::Refused(Refusal::Unknown));
    assert_eq!(to, &ReplyTo::new(Token::new(5)), "the reply answers the call");
}

#[test]
fn a_workers_hello_reaches_the_fleet() {
    let mut model = model();
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    let requests = stepped(&mut model, &env(1), Event::Hello { channel: Token::new(1), hello });
    assert!(requests.is_empty(), "a hello is not answered: {requests:?}");
    assert_eq!(model.fleet().workers(), 1, "the worker is in contact");
}

/// A forge scripted inline: open issues and their comments, which answers
/// every call at once, remembering the engine's records and outcomes and
/// decoding them back as the protocol layer would.
struct Forge {
    issues: List<Issue>,
    /// The forge's clock, which the world moves.
    now: Time,
    /// The next comment's id, and the next item's number.
    comments: u64,
    numbers: u64,
}

#[derive(Debug)]
struct Issue {
    item: Item,
    labels: Box<[Box<[u8]>]>,
    open: bool,
    updated: Time,
    comments: List<Note>,
}

#[derive(Debug)]
struct Note {
    id: u64,
    author: u64,
    revision: u64,
    mark: api::Mark,
    decoded: Option<crate::boundary::Decoded>,
}

impl Forge {
    fn new() -> Forge {
        Forge { issues: List::with_capacity(16), now: Time::ZERO, comments: 100, numbers: 1 }
    }

    fn issue(&mut self, item: Item) -> Option<&mut Issue> {
        let mut index = None;
        for (at, issue) in self.issues.iter().enumerate() {
            if issue.item == item {
                index = Some(u32::try_from(at).unwrap());
            }
        }
        self.issues.get_mut(index?)
    }

    fn open(&mut self, repository: u32, labels: Box<[Box<[u8]>]>) -> Item {
        let item = Item { repository, number: self.numbers };
        self.numbers += 1;
        let updated = self.now;
        self.issues.push(Issue { item, labels, open: true, updated, comments: List::with_capacity(32) }).unwrap();
        item
    }

    fn post(&mut self, item: Item, author: u64, mark: api::Mark, payload: Option<crate::boundary::Payload>) -> u64 {
        self.comments += 1;
        let id = self.comments;
        let decoded = decoded(id, payload);
        let now = self.now;
        let issue = self.issue(item).expect("a comment is on an issue the forge has");
        issue.updated = now;
        issue.comments.push(Note { id, author, revision: 1, mark, decoded }).unwrap();
        id
    }

    /// A page of a listing: the open or closed issues carrying `label`,
    /// updated since `since`, all on the first page.
    fn listing(
        &self,
        repository: u32,
        state: Option<api::State>,
        label: Option<&[u8]>,
        since: Time,
        page: u32,
    ) -> api::Answer {
        let mut items = List::with_capacity(16);
        for issue in &self.issues {
            let open = match state {
                Some(api::State::Open) => issue.open,
                Some(api::State::Closed) => !issue.open,
                None => true,
            };
            let labelled = match &label {
                Some(label) => carries(&issue.labels, label),
                None => true,
            };
            if issue.item.repository != repository || !open || !labelled || issue.updated < since || page > 1 {
                continue;
            }
            items.push(summary(issue)).unwrap();
        }
        api::Answer::Items { items: items.into_boxed(), more: false, now: self.now }
    }

    /// An issue and its comments above `after`, the engine's payloads among
    /// them decoded into `decoded`.
    fn read(
        &mut self,
        item: Item,
        after: u64,
        decoded: &mut List<crate::boundary::Decoded>,
    ) -> Result<api::Answer, api::Error> {
        let Some(issue) = self.issue(item) else { return Err(api::Error::Missing) };
        let mut comments = List::with_capacity(32);
        for note in &issue.comments {
            if note.id <= after {
                continue;
            }
            if let Some(found) = &note.decoded {
                decoded.push(found.clone()).unwrap();
            }
            let comment = api::Comment {
                id: note.id,
                author: note.author,
                created: Time::ZERO,
                revision: note.revision,
                mark: copy_mark(&note.mark),
                body: Box::new([]),
            };
            comments.push(comment).unwrap();
        }
        Ok(api::Answer::Item { item: summary(issue), comments: comments.into_boxed(), more: false })
    }

    /// The answer to `op` on `repository`, and what is decoded inside it.
    fn answer(
        &mut self,
        repository: u32,
        op: api::Op,
        payload: Option<crate::boundary::Payload>,
    ) -> (Result<api::Answer, api::Error>, List<crate::boundary::Decoded>) {
        let mut decoded = List::with_capacity(32);
        let answer = match op {
            api::Op::Items { state, label, since, page, .. } => {
                Ok(self.listing(repository, state, label.as_deref(), since, page))
            }
            api::Op::Item { number, after } => self.read(Item { repository, number }, after, &mut decoded),
            api::Op::Permission { .. } => Ok(api::Answer::Permission(api::Permission::Write)),
            api::Op::CreateIssue { labels, .. } => {
                let item = self.open(repository, labels);
                Ok(api::Answer::Created(item.number))
            }
            api::Op::Post { number, key, person, body } => {
                let item = Item { repository, number };
                let mark = match body {
                    api::Body::Record { position, nonce, .. } => api::Mark::Record { position, nonce },
                    api::Body::Text(_) | api::Body::Payload(_) => match key {
                        Some(key) => api::Mark::Key { key, person },
                        None => api::Mark::None,
                    },
                };
                let id = self.post(item, ENGINE, mark, payload);
                Ok(api::Answer::Commented { id, revision: 1 })
            }
            api::Op::EditComment { number, id, body } => {
                let item = Item { repository, number };
                let issue = self.issue(item).expect("an edit is on an issue the forge has");
                let mut revision = 0_u64;
                for at in 0..issue.comments.len() {
                    let note = issue.comments.get_mut(at).unwrap();
                    if note.id == id {
                        note.revision += 1;
                        revision = note.revision;
                        if let api::Body::Record { position, nonce, .. } = body {
                            note.mark = api::Mark::Record { position, nonce };
                        }
                        note.decoded = decoded_of(id, payload.clone());
                    }
                }
                Ok(api::Answer::Edited { revision })
            }
            api::Op::Close { number } => {
                let now = self.now;
                if let Some(issue) = self.issue(Item { repository, number }) {
                    issue.open = false;
                    issue.updated = now;
                }
                Ok(api::Answer::Done)
            }
            api::Op::Pages { .. } => Ok(api::Answer::Pages { pages: Box::new([]), next: None }),
            api::Op::Comment { number, id } => {
                let Some(issue) = self.issue(Item { repository, number }) else {
                    return (Err(api::Error::Missing), decoded);
                };
                let mut found = Err(api::Error::Missing);
                for note in &issue.comments {
                    if note.id != id {
                        continue;
                    }
                    if let Some(note) = &note.decoded {
                        decoded.push(note.clone()).unwrap();
                    }
                    found = Ok(api::Answer::Comment(api::Comment {
                        id,
                        author: note.author,
                        created: Time::ZERO,
                        revision: note.revision,
                        mark: copy_mark(&note.mark),
                        body: Box::new([]),
                    }));
                }
                found
            }
            api::Op::Pull { .. }
            | api::Op::PullFor { .. }
            | api::Op::Reviews { .. }
            | api::Op::Statuses { .. }
            | api::Op::Remarks { .. }
            | api::Op::Branch { .. }
            | api::Op::Page { .. } => Err(api::Error::Missing),
            api::Op::AddLabels { .. }
            | api::Op::RemoveLabels { .. }
            | api::Op::OpenPull { .. }
            | api::Op::Merge { .. }
            | api::Op::Review { .. }
            | api::Op::SetReviewers { .. }
            | api::Op::SetDependencies { .. }
            | api::Op::Reopen { .. }
            | api::Op::DeleteBranch { .. }
            | api::Op::PutPage { .. }
            | api::Op::DeletePage { .. } => Ok(api::Answer::Done),
        };
        (answer, decoded)
    }
}

fn carries(labels: &[Box<[u8]>], label: &[u8]) -> bool {
    for carried in labels {
        if **carried == *label {
            return true;
        }
    }
    false
}

fn summary(issue: &Issue) -> api::Summary {
    api::Summary {
        number: issue.item.number,
        kind: api::Kind::Issue,
        state: if issue.open { api::State::Open } else { api::State::Closed },
        author: ALICE,
        key: None,
        labels: issue.labels.clone(),
        title: copy_of(b"title"),
        body: copy_of(b"body"),
        updated: issue.updated,
    }
}

fn copy_mark(mark: &api::Mark) -> api::Mark {
    match mark {
        api::Mark::None => api::Mark::None,
        api::Mark::Key { key, person } => api::Mark::Key { key: key.clone(), person: *person },
        api::Mark::Record { position, nonce } => api::Mark::Record { position: *position, nonce: *nonce },
        api::Mark::Mangled => api::Mark::Mangled,
    }
}

fn decoded(id: u64, payload: Option<crate::boundary::Payload>) -> Option<crate::boundary::Decoded> {
    decoded_of(id, payload)
}

/// What the protocol layer would decode from the comment `id` the payload
/// was written into.
fn decoded_of(id: u64, payload: Option<crate::boundary::Payload>) -> Option<crate::boundary::Decoded> {
    match payload? {
        crate::boundary::Payload::Record(record) => Some(crate::boundary::Decoded::Record { comment: id, record }),
        crate::boundary::Payload::Outcome(posted) => Some(crate::boundary::Decoded::Outcome { comment: id, posted }),
        crate::boundary::Payload::Page(_) => None,
    }
}

/// The engine, the scripted forge, and what reached workers, people and
/// the store, at a time that moves by seconds.
struct World {
    model: Model,
    forge: Forge,
    secs: u64,
    /// Requests for workers, people and the store, in order.
    seen: List<Request>,
    pending: Queue<Event>,
    /// How many more forge calls are answered, if the forge stops
    /// answering: those past it are lost.
    calls: Option<u32>,
}

impl World {
    fn new() -> World {
        World {
            model: model(),
            forge: Forge::new(),
            secs: 1,
            seen: List::with_capacity(1024),
            pending: Queue::with_capacity(1024),
            calls: None,
        }
    }

    fn env(&self) -> Env<Limits> {
        env(self.secs)
    }

    /// Hands `event` in, then settles.
    fn deliver(&mut self, event: Event) {
        self.pending.push(event);
        self.settle();
    }

    /// Routes everything pending, answering the forge's calls and the
    /// store's operations at once, until nothing more happens now.
    fn settle(&mut self) {
        for _ in 0_u32..4096 {
            let env = self.env();
            let mut out = Queue::with_capacity(max_out(&LIMITS));
            if let Some(event) = self.pending.pop() {
                step(&mut self.model, &env, event, &mut out);
            } else if self.model.is_ready() {
                crate::model::resume(&mut self.model, &env, &mut out);
            } else if self.model.is_due(env.now) {
                fire(&mut self.model, &env, &mut out);
            } else {
                self.model.reclaim();
                return;
            }
            self.model.reclaim();
            while let Some(request) = out.pop() {
                self.answer(request);
            }
        }
        panic!("the world settles");
    }

    fn answer(&mut self, request: Request) {
        match request {
            Request::Forge { call, repository, op, payload } => {
                match self.calls {
                    Some(0) => return,
                    Some(left) => self.calls = Some(left - 1),
                    None => {}
                }
                self.forge.now = self.env().now;
                let (result, decoded) = self.forge.answer(repository, op, payload);
                self.pending.push(Event::Answered { call, result, decoded: decoded.into_boxed() });
            }
            Request::Store { owner, op } => {
                let stored = match op {
                    crate::boundary::Store::Get { .. } => crate::boundary::Stored::Got(None),
                    crate::boundary::Store::Put { .. }
                    | crate::boundary::Store::Drop { .. }
                    | crate::boundary::Store::Append { .. }
                    | crate::boundary::Store::Expire { .. } => crate::boundary::Stored::Done,
                };
                self.pending.push(Event::Stored { owner, stored });
            }
            seen @ (Request::Assign { .. }
            | Request::Inbound { .. }
            | Request::Cancel { .. }
            | Request::Relayed { .. }
            | Request::Acknowledge { .. }
            | Request::Refuse { .. }
            | Request::Reply { .. }
            | Request::Deliver { .. }
            | Request::Ended { .. }) => self.seen.push(seen).unwrap(),
        }
    }

    /// The engine restarts: everything in memory is lost, and it starts
    /// cold on the forge as it is.
    fn restart(&mut self) {
        self.model = model();
        self.calls = None;
        self.pending = Queue::with_capacity(1024);
        self.seen = List::with_capacity(1024);
        self.settle();
        assert!(self.model.is_loaded(), "the cold start is done");
    }

    /// A session opened by Alice, on the first item, its first turn
    /// assigned to the worker on channel 1.
    fn session() -> (World, Item) {
        let mut world = World::started();
        let ask = Ask::Open { repository: 0, key: copy_of(b"k1"), title: copy_of(b"hi"), message: copy_of(b"hello") };
        world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(9)), person: ALICE, ask });
        let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
        world.deliver(Event::Hello { channel: Token::new(1), hello });
        let item = Item { repository: 0, number: 1 };
        assert_eq!(assignment(&world.seen).expect("the first turn is assigned").item, item);
        (world, item)
    }

    /// The cold start, on an empty forge.
    fn started() -> World {
        let mut world = World::new();
        world.settle();
        assert!(world.model.is_loaded(), "the cold start is done");
        world
    }
}

#[test]
fn a_cold_start_on_an_empty_forge_tells_the_fleet() {
    let mut world = World::started();
    let mut loaded = false;
    while let Some(fact) = world.model.pop_fact() {
        if fact == crate::facts::Fact::Loaded {
            loaded = true;
        }
    }
    assert!(loaded, "the cold start's end is told");
    assert!(world.seen.is_empty(), "nothing reaches workers or people: {:?}", world.seen.as_slice());
}

#[test]
fn a_session_opened_from_the_web_runs_on_a_worker_and_its_reply_is_applied() {
    let mut world = World::started();
    let ask = Ask::Open { repository: 0, key: copy_of(b"k1"), title: copy_of(b"hi"), message: copy_of(b"hello") };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(9)), person: ALICE, ask });
    let item = Item { repository: 0, number: 1 };
    let opened = replied(&world.seen, Reply::Opened { item });
    assert!(opened, "the person hears the session is open: {:?}", world.seen.as_slice());
    assert_eq!(world.model.work().items(), 1, "the hub holds the session");
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    world.deliver(Event::Hello { channel: Token::new(1), hello });
    let assigned = assignment(&world.seen).expect("the session's first turn is assigned");
    assert_eq!(assigned.item, item);
    assert_eq!(assigned.attempt, 1);
    assert!(!assigned.charter.brief.is_empty(), "the charter carries the brief");
    let outcome = crate::boundary::Outcome::Reply { text: copy_of(b"hello to you") };
    let work = crate::boundary::Work { landed: Box::new([]) };
    let answer = crate::boundary::Answer::Ended { outcome, work };
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer });
    let mut acknowledged = false;
    for request in &world.seen {
        if let Request::Acknowledge { item: of, attempt: 1, .. } = request {
            acknowledged = *of == item;
        }
    }
    assert!(acknowledged, "the answer is acknowledged once durable: {:?}", world.seen.as_slice());
    let issue = world.forge.issue(item).unwrap();
    let mut outcomes = 0_u32;
    for note in &issue.comments {
        if let Some(crate::boundary::Decoded::Outcome { .. }) = note.decoded {
            outcomes += 1;
        }
    }
    assert_eq!(outcomes, 1, "the outcome is posted once");
}

/// The assignment the workers were sent last.
fn assignment(seen: &List<Request>) -> Option<crate::boundary::Assignment> {
    let mut found = None;
    for request in seen {
        if let Request::Assign { assignment, .. } = request {
            found = Some(assignment.clone());
        }
    }
    found
}

/// Whether a person was answered `wanted`.
fn replied(seen: &List<Request>, wanted: Reply) -> bool {
    for request in seen {
        if let Request::Reply { reply, .. } = request
            && *reply == wanted
        {
            return true;
        }
    }
    false
}

/// The answer of a session's turn that replies.
fn replied_answer() -> crate::boundary::Answer {
    let outcome = crate::boundary::Outcome::Reply { text: copy_of(b"hello to you") };
    crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([]) } }
}

fn acknowledged(seen: &List<Request>, item: Item, attempt: u64) -> bool {
    for request in seen {
        if let Request::Acknowledge { item: of, attempt: at, .. } = request
            && *of == item
            && *at == attempt
        {
            return true;
        }
    }
    false
}

#[test]
fn a_restart_adopts_the_claim_a_worker_still_hosts() {
    let (mut world, item) = World::session();
    world.restart();
    assert_eq!(world.model.work().items(), 1, "the session is taken in again");
    assert!(assignment(&world.seen).is_none(), "the claim is adopted, not assigned again");
    let hosted = crate::boundary::Hosted { item, attempt: 1, phase: fleet::Phase::Active };
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([hosted]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    world.deliver(Event::Answer { channel: Token::new(2), item, attempt: 1, answer: replied_answer() });
    assert!(acknowledged(&world.seen, item, 1), "the adopted run's answer is applied: {:?}", world.seen.as_slice());
}

/// The phase the item's record on the forge says.
fn phase(world: &mut World, item: Item) -> Option<work::Phase> {
    let issue = world.forge.issue(item)?;
    let mut phase = None;
    for note in &issue.comments {
        if let Some(crate::boundary::Decoded::Record { record, .. }) = &note.decoded {
            phase = Some(record.lifecycle.phase);
        }
    }
    phase
}

#[test]
fn a_restart_while_applying_applies_the_posted_outcome_again() {
    let (mut world, item) = World::session();
    // The forge answers the outcome's post and the record's move to
    // applying, then nothing more.
    world.calls = Some(3);
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer: replied_answer() });
    let applying = matches_applying(phase(&mut world, item));
    assert!(applying, "the record says the outcome is being applied: {:?}", phase(&mut world, item));
    world.restart();
    assert_eq!(phase(&mut world, item), Some(work::Phase::Waiting), "the application is resumed and committed");
    assert_eq!(world.model.work().items(), 1, "the session is taken in again");
    let posted = {
        let issue = world.forge.issue(item).unwrap();
        let mut outcomes = 0_u32;
        for note in &issue.comments {
            if let Some(crate::boundary::Decoded::Outcome { .. }) = note.decoded {
                outcomes += 1;
            }
        }
        outcomes
    };
    assert_eq!(posted, 1, "a restart posts nothing twice");
}

fn matches_applying(phase: Option<work::Phase>) -> bool {
    match phase {
        Some(work::Phase::Applying { .. }) => true,
        Some(
            work::Phase::Waiting
            | work::Phase::Parked
            | work::Phase::Retrying(_)
            | work::Phase::Claimed
            | work::Phase::Held { .. }
            | work::Phase::Done,
        )
        | None => false,
    }
}
