//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: every scope kept with a full index and as many pages
//! lacking, every call holding as much as it may, and every entry point on
//! the way.

use skein_lib::{Env, Queue, ReplyTo, Time, Token, Wall};
use temper_engine_domain_notes::{
    Author, Change, Domain, Event, Fetched, Item, Limits, Listed, Page, Recall, Reference, Request, Scope, Scopes,
    Wrote, max_out, resume, step, worst_case,
};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    scopes: 3,
    entries: 6,
    name_bytes: 24,
    description_bytes: 64,
    body_bytes: 256,
    references: 3,
    calls: 3,
    lines: 8,
    recalled: 3,
    facts: 16,
};

/// What a step asked for, without the payload: the operations, by owner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    Answer,
    List(Token),
    Fetch(Token),
    Write(Token),
}

/// The notes under `limits`, measured: each step's peak is checked against
/// the worst case, less what it handed out in requests, which their
/// receivers count.
struct Measured {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    calls: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        let meter = Meter::new();
        let domain = Domain::new(&limits);
        let out = Queue::with_capacity(max_out(&limits));
        Measured { domain, env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits }, out, meter, bound, calls: 0 }
    }

    fn step(&mut self, event: Event) -> Vec<Asked> {
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    fn resume(&mut self) -> Vec<Asked> {
        self.meter.start();
        resume(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Vec<Asked> {
        let measured = self.meter.end();
        let mut asked = Vec::new();
        while let Some(request) = self.out.pop() {
            asked.push(match request {
                Request::Indexed { .. }
                | Request::Found { .. }
                | Request::Recalled { .. }
                | Request::Noted { .. }
                | Request::Refused { .. } => Asked::Answer,
                Request::List { owner, .. } => Asked::List(owner),
                Request::Fetch { owner, .. } => Asked::Fetch(owner),
                Request::Create { owner, .. } | Request::Edit { owner, .. } | Request::Delete { owner, .. } => {
                    Asked::Write(owner)
                }
            });
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        asked
    }

    fn reply(&mut self) -> ReplyTo {
        self.calls += 1;
        ReplyTo::new(Token::new(self.calls))
    }

    /// Answers every operation in `asked`, and those they lead to, until
    /// none is left, the calls ready resumed: lists with `revision`, reads
    /// with pages of the limits, writes done.
    fn settle(&mut self, mut asked: Vec<Asked>, revision: u64) {
        loop {
            while self.domain.is_ready() {
                asked.extend(self.resume());
            }
            let Some(next) = asked.pop() else {
                return;
            };
            let more = match next {
                Asked::Answer => Vec::new(),
                Asked::List(owner) => {
                    self.step(Event::Listed { owner, pages: Some(listing(&self.env.limits, revision)) })
                }
                Asked::Fetch(owner) => {
                    let fetched = Fetched::Page { revision, page: page(&self.env.limits) };
                    self.step(Event::Fetched { owner, fetched })
                }
                Asked::Write(owner) => self.step(Event::Wrote { owner, wrote: Wrote::Done { revision } }),
            };
            asked.extend(more);
        }
    }
}

fn bytes(len: u32, byte: u8) -> Box<[u8]> {
    vec![byte; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

/// The `nth` name of the limits' length.
fn name(limits: &Limits, nth: u32) -> Box<[u8]> {
    let mut name = bytes(limits.name_bytes, b'n');
    let digits = nth.to_be_bytes();
    let at = name.len() - digits.len();
    name[at..].copy_from_slice(&digits);
    name
}

/// A page of the limits.
fn page(limits: &Limits) -> Page {
    let references = (0..limits.references).map(|number| Reference { repository: 0, number: u64::from(number) });
    Page {
        description: bytes(limits.description_bytes, b'd'),
        author: Author::Person(1),
        references: references.collect(),
        body: bytes(limits.body_bytes, b'b'),
    }
}

/// A listing of as many pages as a scope's index holds, at `revision`.
fn listing(limits: &Limits, revision: u64) -> Box<[Listed]> {
    (0..limits.entries).map(|nth| Listed { name: name(limits, nth), revision }).collect()
}

fn scope(nth: u32) -> Scope {
    Scope::Repository(nth)
}

/// Fills the notes to their limits: every scope kept with its index full
/// and every page lacking again, every call holding a page of the limits,
/// then recalls reading as many pages as they may; and every entry point's
/// ends on the way.
fn fill(limits: Limits) {
    let mut notes = Measured::new(limits);
    // Every scope kept, its index full: a note keeps it, which lists it,
    // writes, then reads every page listed.
    for nth in 0..limits.scopes {
        let reply_to = notes.reply();
        let note = Event::Note { reply_to, scope: scope(nth), name: name(&limits, 99), change: Change::Remove };
        let asked = notes.step(note);
        notes.settle(asked, 1);
    }
    assert_eq!(notes.domain.scopes(), limits.scopes);
    // Every page lacking again: each scope listed at a revision it does not
    // know, its first read in flight.
    let mut reads = Vec::new();
    for nth in 0..limits.scopes {
        let [Asked::List(owner)] = notes.step(Event::Refresh { scope: scope(nth) })[..] else { panic!("listed") };
        let asked = notes.step(Event::Listed { owner, pages: Some(listing(&limits, 2)) });
        assert!(matches!(asked[..], [Asked::Fetch(_)]), "{asked:?}");
        reads.extend(asked);
    }
    // Every call a note of the limits, waiting behind a read.
    let mut writes = Vec::new();
    for nth in 0..limits.calls {
        let reply_to = notes.reply();
        let change = Change::New(page(&limits));
        let event = Event::Note { reply_to, scope: scope(nth % limits.scopes), name: name(&limits, 50 + nth), change };
        writes.extend(notes.step(event));
    }
    assert!(writes.is_empty(), "the notes wait their turn");
    // At their fullest, the notes hold a fair share of the bound: it is not
    // loose past use.
    let fullest = notes.meter.held();
    assert!(fullest.saturating_mul(3) > notes.bound, "{fullest} held of a worst case of {}", notes.bound);
    let reply_to = notes.reply();
    let refused = notes.step(Event::Index { reply_to, scopes: Scopes { repository: 0, goal: None }, budget: 1 });
    assert_eq!(refused, [Asked::Answer], "refused as busy");
    notes.settle(reads, 2);
    // Recalls reading as many pages as they may, each of the limits; the
    // first keeps the deployment's scope, in the place of a full one,
    // evicted.
    for _ in 0..limits.calls {
        let reply_to = notes.reply();
        let recall =
            Recall::Search { scopes: Scopes { repository: 0, goal: None }, query: Box::from(*b"d"), most: u32::MAX };
        let asked = notes.step(Event::Recall { reply_to, recall });
        assert!(!asked.is_empty(), "the deployment's scope is listed, or a page read");
        notes.settle(asked, 3);
    }
    // Indexes and searches answered at once, from scopes read.
    for budget in [0, 100, u32::MAX] {
        let reply_to = notes.reply();
        let asked = notes.step(Event::Index { reply_to, scopes: Scopes { repository: 1, goal: None }, budget });
        notes.settle(asked, 3);
    }
    let reply_to = notes.reply();
    let asked = notes.step(Event::Search {
        reply_to,
        scopes: Scopes { repository: 0, goal: None },
        query: Box::new([]),
        most: 8,
    });
    notes.settle(asked, 3);
    assert_eq!((notes.domain.calls(), notes.domain.ops()), (0, 0));
}

/// Every terminal's other ends: a listing, a read and a write that fail, a
/// page gone, a write that finds the wiki otherwise, a hint.
fn paths(limits: Limits) {
    let mut notes = Measured::new(limits);
    let reply_to = notes.reply();
    let asked = notes.step(Event::Index {
        reply_to,
        scopes: Scopes { repository: 0, goal: Some(Item { repository: 0, number: 1 }) },
        budget: 64,
    });
    for list in asked {
        let Asked::List(owner) = list else { panic!("listed") };
        let asked = notes.step(Event::Listed { owner, pages: None });
        notes.settle(asked, 1);
    }
    let asked = notes.step(Event::Changed { scope: scope(0), name: name(&limits, 1) });
    let [Asked::Fetch(owner)] = asked[..] else { panic!("read") };
    notes.step(Event::Fetched { owner, fetched: Fetched::Failed });
    let asked = notes.step(Event::Changed { scope: scope(0), name: name(&limits, 1) });
    let [Asked::Fetch(owner)] = asked[..] else { panic!("read") };
    notes.step(Event::Fetched { owner, fetched: Fetched::Gone });
    for wrote in [Wrote::Missing, Wrote::Exists, Wrote::Failed] {
        let reply_to = notes.reply();
        let note =
            Event::Note { reply_to, scope: scope(0), name: name(&limits, 2), change: Change::New(page(&limits)) };
        // The scope could not be listed: it is listed again first.
        let mut asked = notes.step(note);
        let mut written = false;
        while let Some(next) = asked.pop() {
            let more = match next {
                Asked::Write(owner) => {
                    written = true;
                    notes.step(Event::Wrote { owner, wrote })
                }
                Asked::List(owner) => notes.step(Event::Listed { owner, pages: Some(listing(&limits, 4)) }),
                Asked::Fetch(owner) => notes.step(Event::Fetched { owner, fetched: Fetched::Gone }),
                Asked::Answer => Vec::new(),
            };
            asked.extend(more);
        }
        assert!(written);
    }
    // A revision, read afresh first: written over if it has not moved, refused
    // as moved if it has.
    for recalled in [4, 3] {
        let reply_to = notes.reply();
        let change = Change::Revise { page: page(&limits), revision: recalled };
        let asked = notes.step(Event::Note { reply_to, scope: scope(0), name: name(&limits, 3), change });
        let [Asked::Fetch(owner)] = asked[..] else { panic!("read afresh") };
        let asked = notes.step(Event::Fetched { owner, fetched: Fetched::Page { revision: 4, page: page(&limits) } });
        notes.settle(asked, 5);
    }
    for fetched in [Fetched::Failed, Fetched::Gone] {
        let reply_to = notes.reply();
        let recall = Recall::Name { scope: scope(0), name: name(&limits, 3) };
        let asked = notes.step(Event::Recall { reply_to, recall });
        let [Asked::Fetch(owner)] = asked[..] else { panic!("read") };
        assert_eq!(notes.step(Event::Fetched { owner, fetched }), [Asked::Answer]);
    }
    let reply_to = notes.reply();
    let oversized = Event::Search {
        reply_to,
        scopes: Scopes { repository: 0, goal: None },
        query: bytes(limits.description_bytes + 1, b'q'),
        most: 1,
    };
    assert_eq!(notes.step(oversized), [Asked::Answer]);
    assert_eq!((notes.domain.calls(), notes.domain.ops()), (0, 0));
}

#[test]
fn notes_full_to_their_limits_stay_within_their_worst_case() {
    fill(LIMITS);
    fill(Limits { scopes: 6, entries: 16, calls: 6, recalled: 6, ..LIMITS });
    fill(Limits { body_bytes: 8192, description_bytes: 512, references: 16, ..LIMITS });
    fill(Limits { scopes: 1, calls: 1, entries: 1, recalled: 1, lines: 1, ..LIMITS });
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { scopes: 5, entries: 2, ..LIMITS });
}
