use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;

use skein_lib::bytes::find;
use skein_lib::{Duration, ReplyTo, Rng, Time, Token};
use temper_engine_domain_brief::{
    self as brief, Body, Budgets, Commit, Domain, Event, Fact, Fit, Gathered, Item, Keep, Kind, Limits, Part, Read,
    Refusal, Request, Source, Unread, Wanted,
};
use temper_world::{Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::referee::{Briefs, FLOOR, Seen, Served, Stimulus};

/// Room in the brief's output queue beyond what one step may emit. Small, so
/// the loop's flow control is exercised.
const SLACK: u32 = 2;

/// The most lines a trace keeps, and deliveries in flight: a world past
/// either fails with its seed rather than grow.
const MAX_TRACE: usize = 200_000;
const MAX_WIRE: usize = 4096;

/// Every kind of section.
pub const KINDS: [Kind; 10] = [
    Kind::Item,
    Kind::Comments,
    Kind::Dependencies,
    Kind::Ci,
    Kind::Reviews,
    Kind::Pull,
    Kind::Attempts,
    Kind::Plan,
    Kind::Notes,
    Kind::Template,
];

/// What content is made of: letters, spaces and newlines, and characters of
/// two, three and four bytes; never `[`, which starts a cut line.
const ALPHABET: [&str; 12] = ["a", "e", "t", "o", "n", "s", "r", " ", "\n", "é", "→", "𝄞"];

/// The brief's limits in the calm world: budgets that cut some sections, a
/// brief's that cannot hold every section at its own, and more than a
/// brief's budget a read may bring.
pub const LIMITS: Limits = Limits {
    briefs: 3,
    sections: 5,
    items: 4,
    parts: 6,
    read_bytes: 600,
    budgets: Budgets {
        item: 160,
        comments: 240,
        dependencies: 200,
        ci: 240,
        reviews: 160,
        pull: 120,
        attempts: 160,
        plan: 120,
        notes: 160,
        template: 120,
    },
    brief_bytes: 700,
    gather: Duration::from_secs(20),
    facts: 64,
};

/// How a world's briefs, sections and reads ended, by kind, for the sweep.
pub const ENDINGS: [&str; 17] = [
    "rendered",
    "empty",
    "cut",
    "items cut",
    "over total",
    "missing",
    "failed by read",
    "failed by deadline",
    "tie",
    "expired",
    "busy",
    "room",
    "oversized",
    "source cut",
    "read failed",
    "read oversized",
    "late",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub limits: Limits,
    /// The briefs the parent asks for, and the time between them; per mille
    /// those past the limits, the sections that are required, and the lists
    /// of dependencies longer than a source may name.
    pub briefs: u32,
    pub brief_gap: Span,
    pub oversized: u32,
    pub required: u32,
    pub long: u32,
    /// The most parts a source has, and characters in a part.
    pub parts: u32,
    pub part_chars: u32,
    /// How long a read takes; per mille those answered late, and how much
    /// later; those that fail; those answered past the read's bounds; and
    /// those answered exactly as their brief's time runs out.
    pub latency: Span,
    pub late: u32,
    pub lateness: Span,
    pub failures: u32,
    pub overreach: u32,
    pub ties: u32,
}

impl Settings {
    /// A world where nothing goes wrong: briefs of every kind of section,
    /// content a read mostly brings whole, sources that answer in time.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            briefs: 30,
            brief_gap: Span::millis(200, 3000),
            oversized: 0,
            required: 250,
            long: 0,
            parts: 4,
            part_chars: 40,
            latency: Span::millis(5, 200),
            late: 0,
            lateness: Span::millis(0, 0),
            failures: 0,
            overreach: 0,
            ties: 0,
        }
    }

    /// A world of its own for `seed`: limits drawn, to tight corners now
    /// and then; briefs some of which are past the limits, asked faster than
    /// they are answered; content more than a read may bring; sources that
    /// are slow, late past a brief's deadline or just at it, failing and
    /// overreaching, at chances drawn from the seed.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0xB1EF_0000_0000_5EED);
        let mut chance = |most: u64| u32::try_from(rng.below(most + 1)).expect("a chance per mille");
        let floor = u32::try_from(FLOOR).expect("small");
        let sections = 1 + chance(5);
        let tight = chance(1000) < 150;
        let mut budget = || if tight || chance(1000) < 100 { floor } else { floor + chance(300) };
        let budgets = Budgets {
            item: budget(),
            comments: budget(),
            dependencies: budget(),
            ci: budget(),
            reviews: budget(),
            pull: budget(),
            attempts: budget(),
            plan: budget(),
            notes: budget(),
            template: budget(),
        };
        let limits = Limits {
            briefs: 1 + chance(3),
            sections,
            items: 1 + chance(4),
            parts: 1 + chance(7),
            read_bytes: 64 + chance(1500),
            budgets,
            brief_bytes: sections * floor + if tight { chance(20) } else { chance(1200) },
            gather: Duration::from_millis(1000 + u64::from(chance(40_000))),
            facts: 64,
        };
        Settings {
            seed,
            limits,
            briefs: 20 + chance(40),
            brief_gap: Span::millis(1, 100 + u64::from(chance(5000))),
            oversized: chance(100),
            required: chance(600),
            long: chance(300),
            parts: 1 + chance(10),
            part_chars: 1 + chance(300),
            latency: Span::millis(1, 10 + u64::from(chance(5000))),
            late: chance(200),
            lateness: Span::millis(0, u64::from(chance(60_000))),
            failures: chance(150),
            overreach: chance(50),
            ties: chance(100),
        }
    }
}

/// What the world counted, as it crossed the boundary.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// The briefs asked for, and how briefs, sections and reads ended.
    pub briefs: u32,
    pub endings: BTreeMap<&'static str, u32>,
    /// Reads served, and those the source answered late.
    pub reads: u32,
    pub lates: u32,
    /// The brief's facts.
    pub facts: u32,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// The parent asks for its next brief.
    Ask,
    /// A read's terminal reaches the brief.
    Answer { owner: u64, read: Read },
}

/// What the world keeps of a brief asked for, until it is answered: when its
/// time runs out, once it is taken in, and the bytes of content its reads
/// brought in time.
#[derive(Debug)]
struct Asked {
    deadline: Option<Time>,
    gathered: usize,
}

/// What the world keeps of a read: its brief, its section, and the most
/// bytes it may bring.
#[derive(Clone, Copy, Debug)]
struct Reading {
    brief: u64,
    index: u32,
    bytes: u32,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    domain: Domain,
    stage: Stage<Limits, Event, Request>,
    /// The renders on their way to the brief, in order: their brief, and
    /// their sections' sources.
    renders: VecDeque<(u64, Vec<(Source, bool)>)>,

    /// Deliveries in flight, and their count; the count names briefs too.
    wire: Schedule<Delivery>,
    in_flight: usize,
    /// The briefs asked for and not answered; the reads asked for and not
    /// served, by owner, as the step that asked for them saw them; the reads
    /// in flight; and those whose terminal is on its way to the brief.
    briefs: Ledger<u64, Asked>,
    asked: BTreeMap<u64, Reading>,
    reads: Ledger<u64, Reading>,
    answering: BTreeMap<u64, Reading>,
    briefs_left: u32,

    referee: Referee<Briefs>,
    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(brief::worst_case(&settings.limits).is_some(), "the shell refuses limits it cannot provision");
        let max_out = brief::max_out(&settings.limits);
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(settings.seed),
            domain: Domain::new(&settings.limits),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            renders: VecDeque::new(),
            wire: Schedule::new(),
            in_flight: 0,
            briefs: Ledger::new("brief"),
            asked: BTreeMap::new(),
            reads: Ledger::new("read"),
            answering: BTreeMap::new(),
            briefs_left: settings.briefs,
            referee: Referee::new(Briefs::new(settings.limits, Duration::from_millis(1))),
            stats: Stats::default(),
            trace: Trace::default(),
            settings,
        };
        if settings.briefs > 0 {
            world.send(Time::ZERO, Delivery::Ask);
        }
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

    /// What crossed between the brief and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Sections and cut lines the referee judged.
    #[must_use]
    pub fn judged(&self) -> (u64, u64) {
        let briefs = self.referee.expectations();
        (briefs.sections, briefs.cuts)
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics, with the seed, if it takes more than
    /// `iterations`.
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
            self.in_flight -= 1;
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
        while let Some(event) = self.stage.next_event() {
            let render = match &event {
                Event::Render { .. } => {
                    let (brief, sections) = self.renders.pop_front().expect("a render on its way names its brief");
                    self.observe(Seen::Asked { brief, sections });
                    Some(brief)
                }
                Event::Read { owner, read } => {
                    self.served(owner.raw(), read);
                    None
                }
            };
            self.log(format!("brief <- {}", describe(&event)));
            let before = self.stage.out.len();
            brief::step(&mut self.domain, &self.stage.env, event, &mut self.stage.out);
            self.stepped(before, render);
        }
        while self.stage.has_room() && self.domain.is_due(now) {
            let before = self.stage.out.len();
            brief::fire(&mut self.domain, &self.stage.env, &mut self.stage.out);
            self.stepped(before, None);
        }
        // What the steps asked for, at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            self.request(request);
        }
        while let Some(fact) = self.domain.pop_fact() {
            self.stats.facts += 1;
            match fact {
                Fact::Expired { .. } => self.end("expired"),
                Fact::Read { read: Gathered::Late } => self.end("late"),
                Fact::Read { read: Gathered::Got | Gathered::Failed | Gathered::Oversized }
                | Fact::Refused { .. }
                | Fact::Gathering { .. }
                | Fact::Rendered { .. }
                | Fact::Failed { .. } => {}
            }
        }
        // The reclaim point.
        self.domain.reclaim();
    }

    /// A step, or an alarm, has ended: the referee sees what it emitted,
    /// from `before` on, as it was emitted; a render's reads are for
    /// `render`'s sections, in order.
    fn stepped(&mut self, before: u32, render: Option<u64>) {
        let mut seen = Vec::new();
        let mut index = 0;
        for request in self.stage.out.iter().skip(usize::try_from(before).expect("small")) {
            seen.push(match request {
                Request::Read { owner, source, keep, fit, parts, bytes } => {
                    let brief = render.expect("a read is asked by a render");
                    self.asked.insert(owner.raw(), Reading { brief, index, bytes: *bytes });
                    index += 1;
                    let source = source.clone();
                    Seen::Read { brief, index: index - 1, source, keep: *keep, fit: *fit, parts: *parts, bytes: *bytes }
                }
                Request::Rendered { reply_to, sections } => {
                    Seen::Rendered { brief: self.whose(reply_to), sections: sections.to_vec() }
                }
                Request::Failed { reply_to, missing, why } => {
                    Seen::Failed { brief: self.whose(reply_to), missing: *missing, why: *why }
                }
                Request::Refused { reply_to, refusal } => {
                    Seen::Refused { brief: self.whose(reply_to), refusal: *refusal }
                }
                Request::Room => Seen::Room,
            });
        }
        if let Some(brief) = render
            && index > 0
        {
            let deadline = self.now.saturating_add(self.settings.limits.gather);
            self.briefs.get_mut(brief).expect("a brief taken in is asked for").deadline = Some(deadline);
        }
        for seen in seen {
            self.observe(seen);
        }
        self.observe(Seen::Stepped);
    }

    /// The brief a reply is for, among those asked and not answered.
    fn whose(&self, reply_to: &ReplyTo) -> u64 {
        *self.briefs.keys().find(|brief| ReplyTo::new(Token::new(**brief)) == *reply_to).expect("a reply names a brief")
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Ask => self.ask(),
            Delivery::Answer { owner, read } => {
                let reading = self.reads.end(owner);
                self.answering.insert(owner, reading);
                self.stage.push(Event::Read { owner: Token::new(owner), read });
            }
        }
    }

    /// The parent asks for a brief of sections drawn, some required, past
    /// the limits now and then, with lists longer than a source may name.
    fn ask(&mut self) {
        self.briefs_left -= 1;
        if self.briefs_left > 0 {
            let at = self.now.saturating_add(self.settings.brief_gap.draw(&mut self.rng));
            self.send(at, Delivery::Ask);
        }
        self.stats.briefs += 1;
        let brief = self.wire.name();
        let limits = self.settings.limits;
        let oversized = self.rng.chance(self.settings.oversized);
        let count = if oversized { limits.sections + 1 } else { self.below(limits.sections + 1) };
        let mut wanted = Vec::new();
        let mut sections = Vec::new();
        for _ in 0..count {
            let kind = KINDS[self.pick(KINDS.len())];
            let items = if self.rng.chance(self.settings.long) {
                limits.items + 1 + self.below(3)
            } else {
                1 + self.below(limits.items)
            };
            let required = self.rng.chance(self.settings.required);
            let source = self.source(kind, items);
            sections.push((source.clone(), required));
            wanted.push(Wanted { source, required });
        }
        self.briefs.open(brief, Asked { deadline: None, gathered: 0 });
        self.renders.push_back((brief, sections));
        let event = Event::Render { reply_to: ReplyTo::new(Token::new(brief)), sections: wanted.into() };
        self.stage.push(event);
    }

    /// A source of `kind`, naming `items` items if it names a list of them.
    fn source(&mut self, kind: Kind, items: u32) -> Source {
        let item = Item { repository: self.below(3), number: self.rng.between(1, 99) };
        let mut head = [0; 32];
        head[..8].copy_from_slice(&self.rng.below(1000).to_be_bytes());
        let head = Commit(head);
        match kind {
            Kind::Item => Source::Item(item),
            Kind::Comments => Source::Comments { item, since: self.rng.below(50) },
            Kind::Dependencies => {
                Source::Dependencies((1..=items).map(|number| Item { repository: 0, number: number.into() }).collect())
            }
            Kind::Ci => Source::Ci { item, head },
            Kind::Reviews => Source::Reviews { item, head },
            Kind::Pull => Source::Pull { item, head },
            Kind::Attempts => Source::Attempts(item),
            Kind::Plan => Source::Plan { goal: item },
            Kind::Notes => {
                let goal = Item { repository: self.below(3), number: 7 };
                Source::Notes { repository: item.repository, goal: self.rng.chance(500).then_some(goal) }
            }
            Kind::Template => Source::Template(self.below(4)),
        }
    }

    /// A read's terminal reaches the brief: the referee sees what it
    /// brought, and the world counts what it judges in time.
    fn served(&mut self, owner: u64, read: &Read) {
        let Reading { brief, index, bytes } =
            self.answering.remove(&owner).expect("a terminal on its way names its read");
        let parts = self.settings.limits.parts as usize;
        let now = self.now;
        let seen = match read {
            Read::Got(content) => {
                let length: usize = content.iter().map(|part| part.bytes.len()).sum();
                if let Some(asked) = self.briefs.get_mut(brief)
                    && content.len() <= parts
                    && length <= bytes as usize
                {
                    asked.gathered += length;
                    if asked.deadline == Some(now) {
                        *self.stats.endings.entry("tie").or_default() += 1;
                    }
                }
                Served::Content(content.to_vec())
            }
            Read::Failed => Served::Failed,
        };
        self.observe(Seen::Served { brief, index, read: seen });
    }

    /// Takes `request` from the brief's queue: an answer, or a read to serve.
    fn request(&mut self, request: Request) {
        self.log(format!("brief -> {}", describe_request(&request)));
        match request {
            Request::Rendered { reply_to, sections } => {
                let asked = self.briefs.end(reply_to.into_token().raw());
                self.end("rendered");
                if sections.is_empty() {
                    self.end("empty");
                }
                if asked.gathered > self.settings.limits.brief_bytes as usize {
                    self.end("over total");
                }
                for section in &sections {
                    match &section.body {
                        Body::Text(text) => {
                            if find(text, b" bytes cut]").is_some() {
                                self.end("cut");
                            }
                            if find(text, b" items cut]").is_some() {
                                self.end("items cut");
                            }
                        }
                        Body::Missing(_) => self.end("missing"),
                    }
                }
            }
            Request::Failed { reply_to, missing: _, why } => {
                self.briefs.end(reply_to.into_token().raw());
                self.end(match why {
                    Unread::Late => "failed by deadline",
                    Unread::Failed | Unread::Oversized => "failed by read",
                });
            }
            Request::Refused { reply_to, refusal } => {
                self.briefs.end(reply_to.into_token().raw());
                self.end(match refusal {
                    Refusal::Busy => "busy",
                    Refusal::Oversized => "oversized",
                });
            }
            Request::Room => self.end("room"),
            Request::Read { owner, source, keep, fit, parts, bytes } => {
                let reading = self.asked.remove(&owner.raw()).expect("a read was seen as it was asked for");
                self.reads.open(owner.raw(), reading);
                let notes = source.kind() == Kind::Notes;
                self.serve(owner.raw(), reading, notes, keep, fit, parts, bytes);
            }
        }
    }

    /// The source of a read answers after a latency, or just as its brief's
    /// time runs out: content drawn, fitted to the read's bounds as it asks;
    /// or past them; or failing.
    #[expect(clippy::too_many_arguments, reason = "a read's every term")]
    fn serve(&mut self, owner: u64, reading: Reading, notes: bool, keep: Keep, fit: Fit, parts: u32, bytes: u32) {
        self.stats.reads += 1;
        let mut back = self.settings.latency.draw(&mut self.rng);
        if self.rng.chance(self.settings.late) {
            self.stats.lates += 1;
            back = back.saturating_add(self.settings.lateness.draw(&mut self.rng));
        }
        let deadline = self.briefs.get(reading.brief).and_then(|asked| asked.deadline);
        let at = match deadline {
            Some(deadline) if self.rng.chance(self.settings.ties) => deadline,
            Some(_) | None => self.now.saturating_add(back),
        };
        let content = if notes { self.index() } else { self.content() };
        let read = if self.rng.chance(self.settings.failures) {
            self.end("read failed");
            Read::Failed
        } else if self.rng.chance(self.settings.overreach) {
            self.end("read oversized");
            let mut over = content;
            over.resize(parts as usize + 1, String::new());
            Read::Got(over.into_iter().map(|part| Part { bytes: part.into_bytes().into(), left: 0 }).collect())
        } else {
            let fitted = match fit {
                Fit::Run => run(content, keep, parts as usize, bytes as usize),
                Fit::Each => each(content, keep, parts as usize, bytes as usize),
                Fit::Lines => lines(content, parts as usize, bytes as usize),
            };
            if fitted.iter().any(|part| part.left > 0) {
                self.end("source cut");
            }
            Read::Got(fitted.into())
        };
        self.send(at, Delivery::Answer { owner, read });
    }

    /// A section's content as its source has it: parts of text drawn.
    fn content(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        for _ in 0..self.below(self.settings.parts + 1) {
            parts.push(self.text(self.settings.part_chars));
        }
        parts
    }

    /// A notes index: a line per entry, and last how many did not fit, if
    /// any did not.
    fn index(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        for _ in 0..self.below(self.settings.parts + 1) {
            let words = self.text(20).replace('\n', " ");
            parts.push(format!("- {words}\n"));
        }
        let more = self.below(4);
        parts.push(if more > 0 { format!("{more} more\n") } else { String::new() });
        parts
    }

    fn text(&mut self, most: u32) -> String {
        let mut text = String::new();
        for _ in 0..self.below(most + 1) {
            text.push_str(ALPHABET[self.pick(ALPHABET.len())]);
        }
        text
    }

    fn send(&mut self, at: Time, delivery: Delivery) {
        self.in_flight += 1;
        assert!(
            self.in_flight <= MAX_WIRE,
            "seed {}: no more than {MAX_WIRE} deliveries in flight",
            self.settings.seed
        );
        self.wire.send(at, delivery);
    }

    fn log(&mut self, line: String) {
        assert!(
            self.trace.lines().len() < MAX_TRACE,
            "seed {}: a trace keeps no more than {MAX_TRACE} lines",
            self.settings.seed
        );
        self.trace.log(self.now, line);
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

    fn below(&mut self, bound: u32) -> u32 {
        u32::try_from(self.rng.below(u64::from(bound))).expect("below a u32")
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.rng.below(u64::try_from(len).expect("small"))).expect("small")
    }

    fn has_work_now(&self) -> bool {
        self.stage.has_events()
            || self.wire.is_due(self.now)
            || self.referee.is_due(self.now)
            || self.domain.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.referee.next_deadline(), self.domain.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty() && !self.stage.has_events(), "nothing is on its way");
        assert!(self.renders.is_empty() && self.asked.is_empty() && self.answering.is_empty(), "nothing is pending");
        self.briefs.assert_settled();
        self.reads.assert_settled();
        assert_eq!(self.domain.briefs(), 0, "the brief holds no brief");
        assert_eq!(self.domain.reads(), 0, "the brief holds no read");
        assert_eq!(self.domain.next_deadline(), None, "no deadline runs");
        self.referee.assert_passed(self.settings.seed);
    }
}

/// What the referee injects: nothing, in this world.
fn inject(stimulus: &Stimulus) {
    match *stimulus {}
}

/// The first `want` bytes of `part`, fewer so as not to split a character.
fn head(part: &str, want: usize) -> usize {
    (0..=want.min(part.len())).rev().find(|at| part.is_char_boundary(*at)).expect("0 is a boundary")
}

/// Where the last `want` bytes of `part` start, later so as not to split a
/// character.
fn tail(part: &str, want: usize) -> usize {
    let len = part.len();
    (len - want.min(len)..=len).find(|at| part.is_char_boundary(*at)).expect("the end is a boundary")
}

/// `part` cut to `want` bytes at the end `keep` does not keep.
fn cut(part: &str, keep: Keep, want: usize) -> Part {
    let (start, end) = match keep {
        Keep::Start => (0, head(part, want)),
        Keep::End => (tail(part, want), part.len()),
    };
    Part { bytes: part.as_bytes()[start..end].into(), left: (part.len() - (end - start)) as u64 }
}

/// The parts of `parts` kept within `most` of them, from the end `keep`
/// names, and the bytes of those left out whole, which go into the `left`
/// of the part next to them.
fn within(parts: Vec<String>, keep: Keep, most: usize) -> (Vec<String>, u64) {
    let count = parts.len();
    let mut dropped = 0;
    let mut kept = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        let keeps = match keep {
            Keep::Start => index < most,
            Keep::End => count - index <= most,
        };
        if keeps {
            kept.push(part);
        } else {
            dropped += part.len() as u64;
        }
    }
    (kept, dropped)
}

/// Tells `dropped` bytes left out whole in the part next to them.
fn tell(mut parts: Vec<Part>, keep: Keep, dropped: u64) -> Vec<Part> {
    let next = match keep {
        Keep::Start => parts.last_mut(),
        Keep::End => parts.first_mut(),
    };
    if let Some(part) = next {
        part.left += dropped;
    }
    parts
}

/// `parts` fitted as one run of bytes from the end `keep` names.
fn run(parts: Vec<String>, keep: Keep, most_parts: usize, most_bytes: usize) -> Vec<Part> {
    let (parts, dropped) = within(parts, keep, most_parts);
    let mut room = most_bytes;
    let mut fitted = Vec::new();
    let order: Vec<String> = match keep {
        Keep::Start => parts,
        Keep::End => parts.into_iter().rev().collect(),
    };
    for part in order {
        let piece = cut(&part, keep, room);
        room -= piece.bytes.len();
        fitted.push(piece);
    }
    if keep == Keep::End {
        fitted.reverse();
    }
    tell(fitted, keep, dropped)
}

/// `parts` fitted each to an even share of the bytes, from the end `keep`
/// names.
fn each(parts: Vec<String>, keep: Keep, most_parts: usize, most_bytes: usize) -> Vec<Part> {
    let (parts, dropped) = within(parts, keep, most_parts);
    let share = most_bytes / parts.len().max(1);
    let fitted = parts.iter().map(|part| cut(part, keep, share)).collect();
    tell(fitted, keep, dropped)
}

/// A notes index fitted by whole lines from the first, keeping its last
/// part, and the lines left out told in an empty part before it.
fn lines(parts: Vec<String>, most_parts: usize, most_bytes: usize) -> Vec<Part> {
    let mut parts = parts;
    let trailer = parts.pop().unwrap_or_default();
    let mut room = most_bytes.saturating_sub(trailer.len());
    let mut fitted = Vec::new();
    let mut dropped = 0;
    for line in parts {
        if dropped == 0 && fitted.len() + 2 < most_parts && line.len() <= room {
            room -= line.len();
            fitted.push(Part { bytes: line.into_bytes().into(), left: 0 });
        } else {
            dropped += line.len() as u64;
        }
    }
    if dropped > 0 {
        fitted.push(Part { bytes: Box::new([]), left: dropped });
    }
    fitted.push(Part { bytes: trailer.into_bytes().into(), left: 0 });
    fitted
}

fn describe(event: &Event) -> String {
    match event {
        Event::Render { reply_to, sections } => format!("render {reply_to:?} {} sections", sections.len()),
        Event::Read { owner, read } => match read {
            Read::Got(parts) => {
                let bytes: usize = parts.iter().map(|part| part.bytes.len()).sum();
                let left: u64 = parts.iter().map(|part| part.left).sum();
                format!("read {} {} parts {bytes} bytes {left} left", owner.raw(), parts.len())
            }
            Read::Failed => format!("read {} failed", owner.raw()),
        },
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Rendered { reply_to, sections } => {
            let mut line = format!("rendered {reply_to:?}");
            for section in sections {
                let written = match &section.body {
                    Body::Text(text) => write!(line, " {:?}:{}", section.kind, text.len()),
                    Body::Missing(why) => write!(line, " {:?}:missing {why:?}", section.kind),
                };
                written.expect("a string takes what is written");
            }
            line
        }
        Request::Failed { reply_to, missing, why } => format!("failed {reply_to:?} {missing:?} {why:?}"),
        Request::Refused { reply_to, refusal } => format!("refused {reply_to:?} {refusal:?}"),
        Request::Room => "room".to_owned(),
        Request::Read { owner, source, keep, fit, parts, bytes } => {
            format!("read {} {:?} {keep:?} {fit:?} {parts} {bytes}", owner.raw(), source.kind())
        }
    }
}
