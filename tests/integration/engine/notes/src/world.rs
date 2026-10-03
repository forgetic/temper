use std::collections::BTreeMap;

use temper_engine_domain_notes::{
    self as notes, Author, Change, Domain, Event, Fact, Fetched, Item, Limits, Listed, Noted, Page, Recall, Reference,
    Refusal, Request, Scope, Scopes, Wrote,
};
use temper_lib::{Duration, ReplyTo, Rng, Time, Token};
use temper_world::{Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::referee::{Notes, Seen, Stimulus};

/// Room in the notes' output queue beyond what one step may emit. Small, so
/// the loop's flow control is exercised.
const SLACK: u32 = 2;

/// The most lines a trace keeps, and deliveries in flight: a run past them
/// fails, with its seed, rather than grow.
const MAX_TRACE: usize = 200_000;
const MAX_DELIVERIES: usize = 10_000;

/// The scopes of the world: the deployment's, two repositories', and a goal
/// in each.
pub const SCOPES: [Scope; 5] = [
    Scope::Deployment,
    Scope::Repository(0),
    Scope::Repository(1),
    Scope::Goal { repository: 0, number: 7 },
    Scope::Goal { repository: 1, number: 9 },
];

/// The runs whose notes are asked for: one of them under a goal in the
/// other repository.
pub const RUNS: [Scopes; 5] = [
    Scopes { repository: 0, goal: None },
    Scopes { repository: 0, goal: Some(Item { repository: 0, number: 7 }) },
    Scopes { repository: 1, goal: None },
    Scopes { repository: 1, goal: Some(Item { repository: 1, number: 9 }) },
    Scopes { repository: 1, goal: Some(Item { repository: 0, number: 7 }) },
];

/// What pages may be called: few, so that a page deleted is made again, and
/// the notes write what people write too.
const NAMES: [&[u8]; 6] = [b"build", b"flaky", b"style", b"retry", b"cache", b"review"];

/// What descriptions are made of, and what searches look for.
const WORDS: [&[u8]; 6] = [b"test", b"build", b"style", b"retry", b"cache", b"slow"];

/// The notes' limits in the calm world: fewer scopes kept than the world
/// has, and fewer entries than a scope may have pages.
pub const LIMITS: Limits = Limits {
    scopes: 4,
    entries: 5,
    name_bytes: 16,
    description_bytes: 48,
    body_bytes: 64,
    references: 2,
    calls: 4,
    lines: 6,
    recalled: 3,
    facts: 64,
};

/// How a world's calls ended, by kind, for the sweep.
pub const ENDINGS: [&str; 14] = [
    "indexed",
    "cut",
    "unread",
    "found",
    "recalled",
    "read failed",
    "noted",
    "missing",
    "exists",
    "moved",
    "unavailable",
    "busy",
    "oversized",
    "evicted",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub limits: Limits,
    /// The calls the parent makes, and the time between them; per mille,
    /// those that are searches, recalls by name, recalls by search and notes
    /// (the rest are indexes); and those past the limits.
    pub calls: u32,
    pub call_gap: Span,
    pub searches: u32,
    pub recalls: u32,
    pub recall_searches: u32,
    pub notes: u32,
    pub oversized: u32,
    /// People's edits of the wiki, the time between them, and per mille
    /// those that delete a page, of those that find one.
    pub edits: u32,
    pub edit_gap: Span,
    pub deletes: u32,
    /// The pages each scope starts with, at most.
    pub pages: u32,
    /// How long a request takes to reach the wiki, and its answer to come
    /// back; per mille those answered late, and how much later; and those
    /// that fail.
    pub latency: Span,
    pub late: u32,
    pub lateness: Span,
    pub failures: u32,
    /// Per mille the edits a webhook hints at, and how long after; and the
    /// time between the parent's polls.
    pub hinted: u32,
    pub hint_after: Span,
    pub poll: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: a few calls of every kind, people
    /// who edit now and then, every edit hinted, a wiki that answers in
    /// time.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            calls: 40,
            call_gap: Span::millis(100, 2000),
            searches: 150,
            recalls: 150,
            recall_searches: 150,
            notes: 200,
            oversized: 0,
            edits: 20,
            edit_gap: Span::millis(500, 5000),
            deletes: 300,
            pages: 3,
            latency: Span::millis(5, 100),
            late: 0,
            lateness: Span::millis(0, 0),
            failures: 0,
            hinted: 1000,
            hint_after: Span::millis(10, 500),
            poll: Duration::from_secs(30),
        }
    }

    /// A world of its own for `seed`: calls of every kind, some past the
    /// limits, in more scopes than are kept; people who make, edit and
    /// delete pages, not always hinted; a wiki that is slow, late and fails,
    /// at chances drawn from the seed.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0x5EED_0000_0000_7E5E);
        let mut chance = |most: u64| u32::try_from(rng.below(most + 1)).expect("a chance per mille");
        let calm = Settings::calm(seed);
        Settings {
            calls: 30 + chance(50),
            call_gap: Span::millis(1, 100 + u64::from(chance(3000))),
            oversized: chance(50),
            edits: 10 + chance(40),
            edit_gap: Span::millis(1, 200 + u64::from(chance(5000))),
            deletes: 200 + chance(400),
            pages: chance(6),
            latency: Span::millis(1, 10 + u64::from(chance(2000))),
            late: chance(200),
            lateness: Span::millis(0, u64::from(chance(20_000))),
            failures: chance(150),
            hinted: chance(1000),
            hint_after: Span::millis(1, 100 + u64::from(chance(10_000))),
            poll: Duration::from_millis(2000 + u64::from(chance(60_000))),
            limits: Limits { scopes: 2 + chance(3), calls: 2 + chance(6), ..LIMITS },
            ..calm
        }
    }
}

/// What the world counted, as it crossed the boundary.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// The calls made, and how each kind ended.
    pub calls: u32,
    pub endings: BTreeMap<&'static str, u32>,
    /// Lines answered, and entries recalled.
    pub lines: u32,
    pub entries: u32,
    /// Wiki operations served, by kind; those that failed, and those late.
    pub lists: u32,
    pub fetches: u32,
    pub writes: u32,
    pub failures: u32,
    pub lates: u32,
    /// People's edits, those that deleted a page, and the hints sent.
    pub edits: u32,
    pub deletes: u32,
    pub hints: u32,
    pub polls: u32,
    /// The notes' facts.
    pub facts: u32,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// The parent makes its next call.
    Call,
    /// A person edits the wiki.
    Edit,
    /// The parent polls: every scope is refreshed.
    Poll,
    /// A request reaches the wiki, which serves it.
    Serve { owner: u64, op: Op },
    /// A terminal reaches the notes, a listing's with its name.
    Answer { event: Event, listing: Option<u64> },
    /// A webhook's hint reaches the notes.
    Hint { scope: Scope, name: Vec<u8> },
}

/// A wiki operation the notes asked for.
#[derive(Debug)]
enum Op {
    List(Scope),
    Fetch(Scope, Box<[u8]>),
    Create(Scope, Box<[u8]>, Page),
    Edit(Scope, Box<[u8]>, Page),
    Delete(Scope, Box<[u8]>),
}

/// What a call asked, as the parent keeps it until its answer: a recall by
/// name, which page.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Asked {
    Index,
    Search,
    Recall(Option<(Scope, Vec<u8>)>),
    Note,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    domain: Domain,
    stage: Stage<Limits, Event, Request>,

    /// Deliveries in flight, whose count names calls too.
    wire: Schedule<Delivery>,
    /// The wiki: each page's revision and content, by scope and name; and
    /// the count that names revisions and listings.
    wiki: BTreeMap<(Scope, Vec<u8>), (u64, Page)>,
    revisions: u64,
    listings: u64,
    /// The calls in flight, the wiki operations in flight; the listings on
    /// their way to the notes, by their operation; and those the notes took
    /// in during this iteration, each with how many requests its steps had
    /// emitted before.
    calls: Ledger<u64, Asked>,
    ops: Ledger<u64, ()>,
    listings_of: BTreeMap<u64, u64>,
    taken_in: Vec<(usize, u64)>,
    /// The pages the last lines answered name.
    last_lines: Vec<(Scope, Vec<u8>)>,
    calls_left: u32,
    edits_left: u32,

    referee: Referee<Notes>,
    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(notes::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let max_out = notes::max_out(&settings.limits);
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(settings.seed),
            domain: Domain::new(&settings.limits),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            wire: Schedule::new(),
            wiki: BTreeMap::new(),
            revisions: 0,
            listings: 0,
            calls: Ledger::new("call"),
            ops: Ledger::new("wiki operation"),
            listings_of: BTreeMap::new(),
            taken_in: Vec::new(),
            last_lines: Vec::new(),
            calls_left: settings.calls,
            edits_left: settings.edits,
            referee: Referee::new(Notes::new(recall_within(&settings))),
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        for scope in SCOPES {
            for _ in 0..world.rng.below(u64::from(settings.pages) + 1) {
                let name = NAMES[world.pick(NAMES.len())];
                let page = world.page(Author::Person(1));
                world.put(scope, name, page);
            }
        }
        world.wire.send(Time::ZERO, Delivery::Call);
        world.wire.send(Time::ZERO, Delivery::Edit);
        world.wire.send(Time::ZERO.saturating_add(settings.poll), Delivery::Poll);
        world
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats.clone()
    }

    /// What crossed between the notes and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Lines and entries the referee judged.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        let notes = self.referee.expectations();
        (notes.lines, notes.entries)
    }

    /// The pages of `scope` as the wiki holds them, by name.
    #[must_use]
    pub fn pages(&self, scope: Scope) -> Vec<Vec<u8>> {
        self.wiki.keys().filter(|(of, _)| *of == scope).map(|(_, name)| name.clone()).collect()
    }

    /// Asks the notes for the index of `scopes` once the world has settled,
    /// lets it settle again, and returns the pages its lines name, by scope.
    pub fn index(&mut self, scopes: Scopes, iterations: u32) -> Vec<(Scope, Vec<u8>)> {
        let token = self.wire.name();
        self.calls.open(token, Asked::Index);
        self.observe(Seen::Asked { call: token, page: None });
        self.stage.push(Event::Index { reply_to: ReplyTo::new(Token::new(token)), scopes, budget: u32::MAX });
        self.last_lines.clear();
        self.run(iterations);
        std::mem::take(&mut self.last_lines)
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics if it takes more than `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            let Some(next) = self.next_time() else {
                self.assert_settled();
                return;
            };
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("seed {}: the world did not settle in {iterations} iterations", self.settings.seed);
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        let now = self.now;
        self.stage.tick(now);
        while let Some(delivery) = self.wire.next(now) {
            self.deliver(delivery);
        }
        if self.referee.is_due(now) {
            let mut stimuli = Vec::new();
            self.referee.fire(now, &mut stimuli);
            self.referee.assert_holding(self.settings.seed);
            for stimulus in &stimuli {
                inject(stimulus);
            }
        }
        while self.stage.has_room() && self.domain.is_ready() {
            notes::resume(&mut self.domain, &self.stage.env, &mut self.stage.out);
        }
        while let Some(event) = self.stage.next_event() {
            // A listing is taken in as the notes step it: the answers emitted
            // before it are judged against the listing before.
            let listing = match &event {
                Event::Listed { owner, pages: Some(_) } => self.listings_of.remove(&owner.raw()),
                Event::Listed { pages: None, .. }
                | Event::Index { .. }
                | Event::Search { .. }
                | Event::Recall { .. }
                | Event::Note { .. }
                | Event::Refresh { .. }
                | Event::Changed { .. }
                | Event::Fetched { .. }
                | Event::Wrote { .. } => None,
            };
            if let Some(listing) = listing {
                self.taken_in.push((usize::try_from(self.stage.out.len()).expect("small"), listing));
            }
            self.log(format!("notes <- {}", describe(&event)));
            notes::step(&mut self.domain, &self.stage.env, event, &mut self.stage.out);
        }
        // What the steps asked for, at the end of the iteration, in the
        // order they asked, each listing taken in among them where it was.
        let mut taken_in = std::mem::take(&mut self.taken_in).into_iter().peekable();
        let mut emitted = 0;
        while let Some(request) = self.stage.out.pop() {
            while let Some((_, listing)) = taken_in.next_if(|(before, _)| *before <= emitted) {
                self.observe(Seen::TakenIn { listing });
            }
            self.request(request);
            emitted += 1;
        }
        for (_, listing) in taken_in {
            self.observe(Seen::TakenIn { listing });
        }
        while let Some(fact) = self.domain.pop_fact() {
            self.stats.facts += 1;
            if fact == (Fact::Kept { evicted: true }) {
                self.end("evicted");
            }
        }
        // The reclaim point.
        self.domain.reclaim();
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Call => self.call(),
            Delivery::Edit => self.edit(),
            Delivery::Poll => {
                self.stats.polls += 1;
                for scope in SCOPES {
                    self.stage.push(Event::Refresh { scope });
                }
                if self.calls_left > 0 || self.edits_left > 0 {
                    self.deliver_at(self.now.saturating_add(self.settings.poll), Delivery::Poll);
                }
            }
            Delivery::Serve { owner, op } => self.serve(owner, op),
            Delivery::Answer { event, listing } => {
                let owner = match &event {
                    Event::Listed { owner, .. } | Event::Fetched { owner, .. } | Event::Wrote { owner, .. } => {
                        owner.raw()
                    }
                    Event::Index { .. }
                    | Event::Search { .. }
                    | Event::Recall { .. }
                    | Event::Note { .. }
                    | Event::Refresh { .. }
                    | Event::Changed { .. } => unreachable!("the wiki answers with terminals"),
                };
                self.ops.end(owner);
                self.assert_contract(&event);
                if let Some(listing) = listing {
                    self.listings_of.insert(owner, listing);
                }
                self.stage.push(event);
            }
            Delivery::Hint { scope, name } => self.stage.push(Event::Changed { scope, name: name.into() }),
        }
    }

    /// The parent makes a call of a kind drawn, for a run drawn.
    fn call(&mut self) {
        self.calls_left -= 1;
        if self.calls_left > 0 {
            let at = self.now.saturating_add(self.settings.call_gap.draw(&mut self.rng));
            self.deliver_at(at, Delivery::Call);
        }
        self.stats.calls += 1;
        let token = self.wire.name();
        let reply_to = ReplyTo::new(Token::new(token));
        let scopes = RUNS[self.pick(RUNS.len())];
        let oversized = self.rng.chance(self.settings.oversized);
        let word = if oversized { vec![b'w'; 49].into_boxed_slice() } else { WORDS[self.pick(WORDS.len())].into() };
        let name: Box<[u8]> = if oversized { vec![b'n'; 17].into() } else { NAMES[self.pick(NAMES.len())].into() };
        let draw = u32::try_from(self.rng.below(1000)).expect("per mille");
        let settings = self.settings;
        let (asked, event) = if draw < settings.searches {
            let most = u32::try_from(self.rng.between(1, 8)).expect("small");
            (Asked::Search, Event::Search { reply_to, scopes, query: word, most })
        } else if draw < settings.searches + settings.recalls {
            let scope = SCOPES[self.pick(SCOPES.len())];
            let page = Some((scope, name.to_vec()));
            (Asked::Recall(page), Event::Recall { reply_to, recall: Recall::Name { scope, name } })
        } else if draw < settings.searches + settings.recalls + settings.recall_searches {
            let most = u32::try_from(self.rng.between(1, 5)).expect("small");
            (Asked::Recall(None), Event::Recall { reply_to, recall: Recall::Search { scopes, query: word, most } })
        } else if draw < settings.searches + settings.recalls + settings.recall_searches + settings.notes {
            let scope = SCOPES[self.pick(SCOPES.len())];
            let change = match self.rng.below(3) {
                0 => Change::New(self.page(Author::Run { repository: 0, number: 7 })),
                1 => {
                    // The revision the run recalled: the page's now, or an
                    // older one now and then, which someone wrote over.
                    let now = self.wiki.get(&(scope, name.to_vec())).map_or(0, |(revision, _)| *revision);
                    let revision = if self.rng.chance(700) { now } else { now.saturating_sub(1) };
                    let page = self.page(Author::Run { repository: 1, number: 9 });
                    Change::Revise { page, revision }
                }
                _ => Change::Remove,
            };
            (Asked::Note, Event::Note { reply_to, scope, name, change })
        } else {
            let budget = u32::try_from(self.rng.below(120)).expect("small");
            (Asked::Index, Event::Index { reply_to, scopes, budget })
        };
        let page = match &asked {
            Asked::Recall(page) => page.clone(),
            Asked::Index | Asked::Search | Asked::Note => None,
        };
        self.calls.open(token, asked);
        self.observe(Seen::Asked { call: token, page });
        self.stage.push(event);
    }

    /// A person makes, edits or deletes a page of a scope drawn, which a
    /// webhook may hint at.
    fn edit(&mut self) {
        self.edits_left -= 1;
        if self.edits_left > 0 {
            let at = self.now.saturating_add(self.settings.edit_gap.draw(&mut self.rng));
            self.deliver_at(at, Delivery::Edit);
        }
        self.stats.edits += 1;
        let scope = SCOPES[self.pick(SCOPES.len())];
        let name = NAMES[self.pick(NAMES.len())];
        if self.wiki.contains_key(&(scope, name.to_vec())) && self.rng.chance(self.settings.deletes) {
            self.stats.deletes += 1;
            self.remove(scope, name);
        } else {
            let page = self.page(Author::Person(2));
            self.put(scope, name, page);
        }
        if self.rng.chance(self.settings.hinted) {
            self.stats.hints += 1;
            let at = self.now.saturating_add(self.settings.hint_after.draw(&mut self.rng));
            self.deliver_at(at, Delivery::Hint { scope, name: name.to_vec() });
        }
    }

    /// The parent's contract (`Event::Listed`, `Event::Fetched`): listings
    /// and pages cut to the notes' limits.
    fn assert_contract(&self, event: &Event) {
        let limits = self.settings.limits;
        let seed = self.settings.seed;
        let fits = |len: usize, most: u32| u32::try_from(len).is_ok_and(|len| len <= most);
        match event {
            Event::Listed { pages: Some(pages), .. } => {
                assert!(fits(pages.len(), limits.entries), "seed {seed}: a listing cut to the notes' entries");
            }
            Event::Fetched { fetched: Fetched::Page { page, .. }, .. } => {
                let within = fits(page.description.len(), limits.description_bytes)
                    && fits(page.body.len(), limits.body_bytes)
                    && fits(page.references.len(), limits.references);
                assert!(within, "seed {seed}: a page cut to the notes' limits");
            }
            Event::Listed { pages: None, .. }
            | Event::Fetched { .. }
            | Event::Wrote { .. }
            | Event::Index { .. }
            | Event::Search { .. }
            | Event::Recall { .. }
            | Event::Note { .. }
            | Event::Refresh { .. }
            | Event::Changed { .. } => {}
        }
    }

    /// What the notes asked for, carried as the parent would.
    fn request(&mut self, request: Request) {
        self.log(format!("notes -> {}", describe_request(&request)));
        match request {
            Request::Indexed { reply_to, lines, more, unread } => {
                let call = reply_to.into_token().raw();
                assert_eq!(self.calls.end(call), Asked::Index, "an index is answered as one");
                self.end("indexed");
                if more > 0 {
                    self.end("cut");
                }
                if unread > 0 {
                    self.end("unread");
                }
                self.lines(lines);
                self.observe(Seen::Answered { call });
            }
            Request::Found { reply_to, lines, .. } => {
                let call = reply_to.into_token().raw();
                assert_eq!(self.calls.end(call), Asked::Search, "a search is answered as one");
                self.end("found");
                self.lines(lines);
                self.observe(Seen::Answered { call });
            }
            Request::Recalled { reply_to, entries, failed } => {
                let call = reply_to.into_token().raw();
                let Asked::Recall(_) = self.calls.end(call) else { panic!("a recall is answered as one") };
                self.end("recalled");
                if failed > 0 {
                    self.end("read failed");
                }
                self.stats.entries += u32::try_from(entries.len()).expect("few");
                self.observe(Seen::Recalled { call, entries: entries.into_vec(), failed });
            }
            Request::Noted { reply_to, noted } => {
                let call = reply_to.into_token().raw();
                assert_eq!(self.calls.end(call), Asked::Note, "a note is answered as one");
                self.end(match noted {
                    Noted::Done => "noted",
                    Noted::Missing => "missing",
                    Noted::Exists => "exists",
                    Noted::Moved => "moved",
                    Noted::Unavailable => "unavailable",
                });
                self.observe(Seen::Answered { call });
            }
            Request::Refused { reply_to, refusal } => {
                let call = reply_to.into_token().raw();
                self.calls.end(call);
                self.end(match refusal {
                    Refusal::Busy => "busy",
                    Refusal::Oversized => "oversized",
                });
                self.observe(Seen::Answered { call });
            }
            Request::List { owner, scope } => self.send(owner, Op::List(scope)),
            Request::Fetch { owner, scope, name } => self.send(owner, Op::Fetch(scope, name)),
            Request::Create { owner, scope, name, page } => self.send(owner, Op::Create(scope, name, page)),
            Request::Edit { owner, scope, name, page, revision: _ } => self.send(owner, Op::Edit(scope, name, page)),
            Request::Delete { owner, scope, name } => self.send(owner, Op::Delete(scope, name)),
        }
    }

    /// Sends the wiki operation `op` of `owner` on its way to the wiki.
    fn send(&mut self, owner: Token, op: Op) {
        self.ops.open(owner.raw(), ());
        let at = self.now.saturating_add(self.settings.latency.draw(&mut self.rng));
        self.deliver_at(at, Delivery::Serve { owner: owner.raw(), op });
    }

    /// Sends `delivery`, due at `at`, within the world's bound on what is in
    /// flight.
    fn deliver_at(&mut self, at: Time, delivery: Delivery) {
        assert!(
            self.wire.len() < MAX_DELIVERIES,
            "seed {}: no more than {MAX_DELIVERIES} deliveries in flight",
            self.settings.seed
        );
        self.wire.send(at, delivery);
    }

    /// Logs `line` in the trace, within the world's bound on it.
    fn log(&mut self, line: String) {
        assert!(
            self.trace.lines().len() < MAX_TRACE,
            "seed {}: a trace of no more than {MAX_TRACE} lines",
            self.settings.seed
        );
        self.trace.log(self.now, line);
    }

    /// The wiki serves `op`, or fails it, and its answer goes back, late
    /// now and then.
    fn serve(&mut self, owner: u64, op: Op) {
        let failed = self.rng.chance(self.settings.failures);
        if failed {
            self.stats.failures += 1;
        }
        let mut listing = None;
        let event = match op {
            Op::List(scope) => {
                self.stats.lists += 1;
                let pages = if failed {
                    None
                } else {
                    self.listings += 1;
                    listing = Some(self.listings);
                    self.observe(Seen::Served { listing: self.listings, scope });
                    // The protocol layer cuts a listing at the notes' limit.
                    let entries = usize::try_from(self.settings.limits.entries).expect("small");
                    let listed: Vec<Listed> = self
                        .wiki
                        .iter()
                        .filter(|((of, _), _)| *of == scope)
                        .take(entries)
                        .map(|((_, name), (revision, _))| Listed { name: name.clone().into(), revision: *revision })
                        .collect();
                    Some(listed.into_boxed_slice())
                };
                Event::Listed { owner: Token::new(owner), pages }
            }
            Op::Fetch(scope, name) => {
                self.stats.fetches += 1;
                let fetched = if failed {
                    Fetched::Failed
                } else {
                    match self.wiki.get(&(scope, name.to_vec())) {
                        Some((revision, page)) => Fetched::Page { revision: *revision, page: page.clone() },
                        None => Fetched::Gone,
                    }
                };
                Event::Fetched { owner: Token::new(owner), fetched }
            }
            Op::Create(scope, name, page) => {
                self.stats.writes += 1;
                let wrote = if failed {
                    Wrote::Failed
                } else if self.wiki.contains_key(&(scope, name.to_vec())) {
                    Wrote::Exists
                } else {
                    Wrote::Done { revision: self.put(scope, &name, page) }
                };
                Event::Wrote { owner: Token::new(owner), wrote }
            }
            Op::Edit(scope, name, page) => {
                self.stats.writes += 1;
                let wrote = if failed {
                    Wrote::Failed
                } else if self.wiki.contains_key(&(scope, name.to_vec())) {
                    Wrote::Done { revision: self.put(scope, &name, page) }
                } else {
                    Wrote::Missing
                };
                Event::Wrote { owner: Token::new(owner), wrote }
            }
            Op::Delete(scope, name) => {
                self.stats.writes += 1;
                let wrote = if failed {
                    Wrote::Failed
                } else if self.wiki.contains_key(&(scope, name.to_vec())) {
                    Wrote::Done { revision: self.remove(scope, &name) }
                } else {
                    Wrote::Missing
                };
                Event::Wrote { owner: Token::new(owner), wrote }
            }
        };
        let mut back = self.settings.latency.draw(&mut self.rng);
        if self.rng.chance(self.settings.late) {
            self.stats.lates += 1;
            back = back.saturating_add(self.settings.lateness.draw(&mut self.rng));
        }
        self.deliver_at(self.now.saturating_add(back), Delivery::Answer { event, listing });
    }

    /// Makes or edits the page `name` of `scope`, and returns its revision.
    fn put(&mut self, scope: Scope, name: &[u8], page: Page) -> u64 {
        self.revisions += 1;
        let revision = self.revisions;
        self.wiki.insert((scope, name.to_vec()), (revision, page.clone()));
        self.observe(Seen::Changed { scope, name: name.to_vec(), page: Some(page) });
        revision
    }

    /// Deletes the page `name` of `scope`.
    fn remove(&mut self, scope: Scope, name: &[u8]) -> u64 {
        self.revisions += 1;
        self.wiki.remove(&(scope, name.to_vec()));
        self.observe(Seen::Changed { scope, name: name.to_vec(), page: None });
        self.revisions
    }

    /// A page of a few words, its body naming its revision to be.
    fn page(&mut self, author: Author) -> Page {
        let mut description = Vec::new();
        for _ in 0..self.rng.between(1, 3) {
            if !description.is_empty() {
                description.push(b' ');
            }
            description.extend_from_slice(WORDS[self.pick(WORDS.len())]);
        }
        let body = format!("as of revision {}", self.revisions + 1).into_bytes();
        let references = vec![Reference { repository: 0, number: self.rng.below(20) }];
        Page { description: description.into(), author, references: references.into(), body: body.into() }
    }

    /// The lines of an answer, for the referee.
    fn lines(&mut self, lines: Box<[notes::Line]>) {
        self.stats.lines += u32::try_from(lines.len()).expect("few");
        self.last_lines = lines.iter().map(|line| (line.scope, line.name.to_vec())).collect();
        self.observe(Seen::Lines { lines: lines.into_vec() });
    }

    fn end(&mut self, ending: &'static str) {
        *self.stats.endings.entry(ending).or_default() += 1;
    }

    /// The referee observes `seen`, which ends the test if it breaks an
    /// expectation.
    fn observe(&mut self, seen: Seen) {
        let mut stimuli = Vec::new();
        self.referee.observe(self.now, seen, &mut stimuli);
        self.referee.assert_holding(self.settings.seed);
        for stimulus in &stimuli {
            inject(stimulus);
        }
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.rng.below(u64::try_from(len).expect("small"))).expect("small")
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events() || self.domain.is_ready() || self.wire.is_due(self.now) || self.referee.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.referee.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty() && !self.stage.has_events(), "nothing is on its way");
        self.calls.assert_settled();
        self.ops.assert_settled();
        assert_eq!(self.domain.calls(), 0, "the notes hold no call");
        assert_eq!(self.domain.ops(), 0, "the notes hold no wiki operation");
        assert!(!self.domain.is_ready(), "no call is ready");
        self.referee.assert_passed(self.settings.seed);
    }
}

/// What the referee injects: nothing, in this world.
fn inject(stimulus: &Stimulus) {
    match *stimulus {}
}

/// How long a recall may take: a pass of each of its scopes behind the
/// writes queued there, then its reads, each operation up to its latency
/// both ways, and late; with room to spare.
fn recall_within(settings: &Settings) -> Duration {
    let limits = settings.limits;
    let operations = u64::from(limits.entries + limits.calls + limits.recalled + 4);
    let each = settings.latency.max.saturating_mul(2).saturating_add(settings.lateness.max);
    each.saturating_mul(operations.saturating_mul(3))
}

fn describe(event: &Event) -> String {
    match event {
        Event::Index { reply_to, scopes, budget } => format!("index {reply_to:?} {scopes:?} {budget}"),
        Event::Search { reply_to, scopes, .. } => format!("search {reply_to:?} {scopes:?}"),
        Event::Recall { reply_to, .. } => format!("recall {reply_to:?}"),
        Event::Note { reply_to, scope, .. } => format!("note {reply_to:?} {scope:?}"),
        Event::Refresh { scope } => format!("refresh {scope:?}"),
        Event::Changed { scope, name } => format!("changed {scope:?} {}", String::from_utf8_lossy(name)),
        Event::Listed { owner, pages } => {
            format!("listed {} {:?}", owner.raw(), pages.as_ref().map(|pages| pages.len()))
        }
        Event::Fetched { owner, fetched } => format!(
            "fetched {} {}",
            owner.raw(),
            match fetched {
                Fetched::Page { revision, .. } => format!("revision {revision}"),
                Fetched::Gone => "gone".to_owned(),
                Fetched::Failed => "failed".to_owned(),
            }
        ),
        Event::Wrote { owner, wrote } => format!("wrote {} {wrote:?}", owner.raw()),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Indexed { reply_to, lines, more, unread } => {
            format!("indexed {reply_to:?} {} more {more} unread {unread}", lines.len())
        }
        Request::Found { reply_to, lines, more, .. } => format!("found {reply_to:?} {} more {more}", lines.len()),
        Request::Recalled { reply_to, entries, failed } => {
            format!("recalled {reply_to:?} {} failed {failed}", entries.len())
        }
        Request::Noted { reply_to, noted } => format!("noted {reply_to:?} {noted:?}"),
        Request::Refused { reply_to, refusal } => format!("refused {reply_to:?} {refusal:?}"),
        Request::List { owner, scope } => format!("list {} {scope:?}", owner.raw()),
        Request::Fetch { owner, scope, name } => {
            format!("fetch {} {scope:?} {}", owner.raw(), String::from_utf8_lossy(name))
        }
        Request::Create { owner, scope, name, .. } => {
            format!("create {} {scope:?} {}", owner.raw(), String::from_utf8_lossy(name))
        }
        Request::Edit { owner, scope, name, .. } => {
            format!("edit {} {scope:?} {}", owner.raw(), String::from_utf8_lossy(name))
        }
        Request::Delete { owner, scope, name } => {
            format!("delete {} {scope:?} {}", owner.raw(), String::from_utf8_lossy(name))
        }
    }
}
