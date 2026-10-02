use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;

use temper_engine_model_brief::{
    self as brief, Body, Budgets, Commit, Event, Fact, Gathered, Item, Keep, Kind, Limits, Model, Part, Read, Request,
    Section, Source, Wanted,
};
use temper_lib::{Duration, ReplyTo, Rng, Time, Token};
use temper_world::{Ledger, Referee, Schedule, Span, Stage, Trace};

use crate::referee::{Briefs, Seen, Served, Stimulus, read_back};

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
pub const ENDINGS: [&str; 13] = [
    "rendered",
    "empty",
    "cut",
    "over total",
    "missing",
    "failed",
    "expired",
    "busy",
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
    /// those past the limits, and the sections that are required.
    pub briefs: u32,
    pub brief_gap: Span,
    pub oversized: u32,
    pub required: u32,
    /// The most parts a source has, and characters in a part.
    pub parts: u32,
    pub part_chars: u32,
    /// How long a read takes; per mille those answered late, and how much
    /// later; those that fail; and those answered past the read's bounds.
    pub latency: Span,
    pub late: u32,
    pub lateness: Span,
    pub failures: u32,
    pub overreach: u32,
}

impl Settings {
    /// A world where nothing goes wrong: briefs of every kind of section,
    /// content that a read brings whole, sources that answer in time.
    #[must_use]
    pub fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            limits: LIMITS,
            briefs: 30,
            brief_gap: Span::millis(200, 3000),
            oversized: 0,
            required: 250,
            parts: 4,
            part_chars: 40,
            latency: Span::millis(5, 200),
            late: 0,
            lateness: Span::millis(0, 0),
            failures: 0,
            overreach: 0,
        }
    }

    /// A world of its own for `seed`: briefs some of which are past the
    /// limits, asked faster than they are answered; content more than a
    /// read may bring; sources that are slow, late past a brief's deadline,
    /// failing and overreaching, at chances drawn from the seed.
    #[must_use]
    pub fn random(seed: u64) -> Settings {
        let mut rng = Rng::new(seed ^ 0xB1EF_0000_0000_5EED);
        let mut chance = |most: u64| u32::try_from(rng.below(most + 1)).expect("a chance per mille");
        let calm = Settings::calm(seed);
        Settings {
            briefs: 20 + chance(40),
            brief_gap: Span::millis(1, 100 + u64::from(chance(5000))),
            oversized: chance(100),
            required: chance(600),
            parts: 1 + chance(10),
            part_chars: 1 + chance(300),
            latency: Span::millis(1, 10 + u64::from(chance(5000))),
            late: chance(200),
            lateness: Span::millis(0, u64::from(chance(60_000))),
            failures: chance(150),
            overreach: chance(50),
            limits: Limits { briefs: 1 + chance(4), ..LIMITS },
            ..calm
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

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    model: Model,
    stage: Stage<Limits, Event, Request>,
    /// The brief each request in the stage's queue was emitted for, if a
    /// render emitted it, in step with the queue; and the briefs of the
    /// renders on their way to the brief, in order.
    labels: VecDeque<Option<u64>>,
    renders: VecDeque<u64>,

    /// Deliveries in flight, and their count; the count names briefs too.
    wire: Schedule<Delivery>,
    in_flight: usize,
    /// The briefs asked for and not answered, with the bytes of content
    /// their reads brought in time; and the reads in flight, by owner, with
    /// the brief and section each is for.
    briefs: Ledger<u64, usize>,
    reads: Ledger<u64, (u64, u32, u32)>,
    /// The reads of each brief seen so far.
    indexes: BTreeMap<u64, u32>,
    /// Reads whose terminal is on its way to the brief: their brief and
    /// section.
    answering: BTreeMap<u64, (u64, u32, u32)>,
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
        let within = settings.limits.gather.saturating_add(Duration::from_millis(1));
        let mut world = World {
            now: Time::ZERO,
            rng: Rng::new(settings.seed),
            model: Model::new(&settings.limits),
            stage: Stage::new(settings.limits, max_out, max_out + SLACK),
            labels: VecDeque::new(),
            renders: VecDeque::new(),
            wire: Schedule::new(),
            in_flight: 0,
            briefs: Ledger::new("brief"),
            reads: Ledger::new("read"),
            indexes: BTreeMap::new(),
            answering: BTreeMap::new(),
            briefs_left: settings.briefs,
            referee: Referee::new(Briefs::new(settings.limits, within)),
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
            let label = match &event {
                Event::Render { .. } => Some(self.renders.pop_front().expect("a render on its way names its brief")),
                Event::Read { owner, read } => {
                    self.served(owner.raw(), read);
                    None
                }
            };
            self.log(format!("brief <- {}", describe(&event)));
            let before = self.stage.out.len();
            brief::step(&mut self.model, &self.stage.env, event, &mut self.stage.out);
            self.label(before, label);
        }
        while self.stage.has_room() && self.model.is_due(now) {
            let before = self.stage.out.len();
            brief::fire(&mut self.model, &self.stage.env, &mut self.stage.out);
            self.label(before, None);
        }
        // What the steps asked for, at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            let label = self.labels.pop_front().expect("a label for each request");
            self.request(request, label);
        }
        while let Some(fact) = self.model.pop_fact() {
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
        self.model.reclaim();
    }

    /// Labels what a step emitted since `before` with the brief it is for.
    fn label(&mut self, before: u32, label: Option<u64>) {
        for _ in before..self.stage.out.len() {
            self.labels.push_back(label);
        }
    }

    /// Hands `delivery` to its destination.
    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Ask => self.ask(),
            Delivery::Answer { owner, read } => {
                let asked = self.reads.end(owner);
                self.answering.insert(owner, asked);
                self.stage.push(Event::Read { owner: Token::new(owner), read });
            }
        }
    }

    /// The parent asks for a brief of sections drawn, some required, past
    /// the limits now and then.
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
        let mut most = 0;
        for _ in 0..count {
            let kind = KINDS[self.pick(KINDS.len())];
            let items = 1 + self.below(limits.items);

            let required = self.rng.chance(self.settings.required);
            let source = self.source(kind, items);
            most = most.max(match &source {
                Source::Dependencies(items) => items.len(),
                Source::Item(_)
                | Source::Comments { .. }
                | Source::Ci { .. }
                | Source::Reviews { .. }
                | Source::Pull { .. }
                | Source::Attempts(_)
                | Source::Plan { .. }
                | Source::Notes { .. }
                | Source::Template(_) => 0,
            });
            sections.push((kind, required));
            wanted.push(Wanted { source, required });
        }
        self.observe(Seen::Asked { brief, sections, items: most });
        self.briefs.open(brief, 0);
        self.renders.push_back(brief);
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
            Kind::Notes => Source::Notes { repository: item.repository, goal: self.rng.chance(500).then_some(item) },
            Kind::Template => Source::Template(self.below(4)),
        }
    }

    /// A read's terminal reaches the brief: the referee sees what it
    /// brought.
    fn served(&mut self, owner: u64, read: &Read) {
        let (brief, index, most) = self.answering.remove(&owner).expect("a terminal on its way names its read");
        let limits = self.settings.limits;
        let read = match read {
            Read::Got(parts) => {
                let bytes: usize = parts.iter().map(|part| part.bytes.len()).sum();
                let within = parts.len() <= usize::try_from(limits.parts).expect("small") && bytes <= most as usize;
                if within {
                    if let Some(gathered) = self.briefs.get_mut(brief) {
                        *gathered += bytes;
                    }
                    Served::Content(parts.to_vec())
                } else {
                    Served::Oversized
                }
            }
            Read::Failed => Served::Failed,
        };
        self.observe(Seen::Served { brief, index, read });
    }

    /// Takes `request` from the brief's queue: an answer, or a read to serve.
    fn request(&mut self, request: Request, label: Option<u64>) {
        self.log(format!("brief -> {}", describe_request(&request)));
        match request {
            Request::Rendered { reply_to, sections } => {
                let brief = reply_to.into_token().raw();
                let gathered = self.briefs.end(brief);
                self.indexes.remove(&brief);
                self.rendered(&sections, gathered);
                self.observe(Seen::Rendered { brief, sections: sections.into_vec() });
            }
            Request::Failed { reply_to, missing, why: _ } => {
                let brief = reply_to.into_token().raw();
                self.briefs.end(brief);
                self.indexes.remove(&brief);
                self.end("failed");
                self.observe(Seen::Failed { brief, missing });
            }
            Request::Refused { reply_to, refusal } => {
                let brief = reply_to.into_token().raw();
                self.briefs.end(brief);
                self.end(match refusal {
                    temper_engine_model_brief::Refusal::Busy => "busy",
                    temper_engine_model_brief::Refusal::Oversized => "oversized",
                });
                self.observe(Seen::Refused { brief, refusal });
            }
            Request::Room => {}
            Request::Read { owner, source: _, keep, fit: _, parts, bytes } => {
                let brief = label.expect("a read is asked by a render");
                let index = self.indexes.entry(brief).or_insert(0);
                let section = *index;
                *index += 1;
                self.reads.open(owner.raw(), (brief, section, bytes));
                self.serve(owner.raw(), keep, parts, bytes);
            }
        }
    }

    /// Counts what a rendered brief shows.
    fn rendered(&mut self, sections: &[Section], gathered: usize) {
        self.end("rendered");
        if sections.is_empty() {
            self.end("empty");
        }
        if gathered > self.settings.limits.brief_bytes as usize {
            self.end("over total");
        }
        for section in sections {
            match &section.body {
                Body::Text(text) => {
                    if read_back(text).is_some_and(|(_, cut)| cut > 0) {
                        self.end("cut");
                    }
                }
                Body::Missing(_) => self.end("missing"),
            }
        }
    }

    /// The source of a read answers after a latency: content drawn, cut to
    /// the read's bounds from the end it keeps; or past them; or failing.
    fn serve(&mut self, owner: u64, keep: Keep, most_parts: u32, most_bytes: u32) {
        self.stats.reads += 1;
        let mut back = self.settings.latency.draw(&mut self.rng);
        if self.rng.chance(self.settings.late) {
            self.stats.lates += 1;
            back = back.saturating_add(self.settings.lateness.draw(&mut self.rng));
        }
        let read = if self.rng.chance(self.settings.failures) {
            self.end("read failed");
            Read::Failed
        } else if self.rng.chance(self.settings.overreach) {
            self.end("read oversized");
            let mut parts = self.content();
            parts.resize(most_parts as usize + 1, String::new());
            Read::Got(parts.into_iter().map(|part| Part { bytes: part.into_bytes().into(), left: 0 }).collect())
        } else {
            let parts = fit(self.content(), keep, most_parts as usize, most_bytes as usize);
            if parts.iter().any(|part| part.left > 0) {
                self.end("source cut");
            }
            Read::Got(parts.into())
        };
        self.send(self.now.saturating_add(back), Delivery::Answer { owner, read });
    }

    /// A section's content as its source has it: parts of text drawn.
    fn content(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        for _ in 0..self.below(self.settings.parts + 1) {
            let mut part = String::new();
            for _ in 0..self.below(self.settings.part_chars + 1) {
                part.push_str(ALPHABET[self.pick(ALPHABET.len())]);
            }
            parts.push(part);
        }
        parts
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
            || self.model.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.referee.next_deadline(), self.model.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert!(self.wire.is_empty() && !self.stage.has_events(), "nothing is on its way");
        assert!(self.labels.is_empty() && self.renders.is_empty() && self.answering.is_empty(), "nothing is pending");
        self.briefs.assert_settled();
        self.reads.assert_settled();
        assert_eq!(self.model.briefs(), 0, "the brief holds no brief");
        assert_eq!(self.model.reads(), 0, "the brief holds no read");
        assert_eq!(self.model.next_deadline(), None, "no deadline runs");
        self.referee.assert_passed(self.settings.seed);
    }
}

/// What the referee injects: nothing, in this world.
fn inject(stimulus: &Stimulus) {
    match *stimulus {}
}

/// `parts` cut to at most `most_parts` parts and `most_bytes` bytes, as a
/// source does: its content kept from the end `keep` names, as one run of
/// bytes, and what it left out told in the `left` of the part next to it.
fn fit(parts: Vec<String>, keep: Keep, most_parts: usize, most_bytes: usize) -> Vec<Part> {
    let count = parts.len();
    let mut room = most_bytes;
    let mut kept: Vec<Part> = Vec::new();
    let mut dropped = 0;
    let mut take = |part: String| {
        let len = part.len();
        let want = room.min(len);
        let (start, end) = match keep {
            Keep::Start => (0, (0..=want).rev().find(|at| part.is_char_boundary(*at)).expect("0 is a boundary")),
            Keep::End => {
                ((len - want..=len).find(|at| part.is_char_boundary(*at)).expect("the end is a boundary"), len)
            }
        };
        room -= end - start;
        Part { bytes: part.as_bytes()[start..end].into(), left: u64::try_from(len - (end - start)).expect("small") }
    };
    match keep {
        Keep::Start => {
            for (index, part) in parts.into_iter().enumerate() {
                if index < most_parts {
                    kept.push(take(part));
                } else {
                    dropped += part.len();
                }
            }
            if let Some(last) = kept.last_mut() {
                last.left += u64::try_from(dropped).expect("small");
            }
        }
        Keep::End => {
            for (index, part) in parts.into_iter().enumerate().rev() {
                if count - index <= most_parts {
                    kept.push(take(part));
                } else {
                    dropped += part.len();
                }
            }
            kept.reverse();
            if let Some(first) = kept.first_mut() {
                first.left += u64::try_from(dropped).expect("small");
            }
        }
    }
    kept
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
        Request::Room => "room".to_owned(),
        Request::Refused { reply_to, refusal } => format!("refused {reply_to:?} {refusal:?}"),
        Request::Read { owner, source, keep, .. } => format!("read {} {:?} {keep:?}", owner.raw(), source.kind()),
    }
}
