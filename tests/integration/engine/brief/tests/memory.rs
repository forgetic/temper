//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: every brief gathering with every section read in
//! full, each rendered at its budgets, and every entry point on the way.

use temper_engine_domain_brief::{
    Budgets, Domain, Event, Item, Limits, Part, Read, Request, Source, Wanted, fire, max_out, step, worst_case,
};
use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    briefs: 3,
    sections: 4,
    items: 5,
    parts: 6,
    read_bytes: 2048,
    budgets: Budgets {
        item: 512,
        comments: 512,
        dependencies: 768,
        ci: 768,
        reviews: 512,
        pull: 256,
        attempts: 512,
        plan: 256,
        notes: 512,
        template: 256,
    },
    brief_bytes: 1600,
    gather: Duration::from_secs(10),
    facts: 16,
};

/// What a step asked for, without the payload: a read, by owner; or an
/// answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    Answer,
    Room,
    Read(Token),
}

/// The brief under `limits`, measured: each step's peak is checked against
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
        Measured { domain, env: Env { now: Time::ZERO, limits }, out, meter, bound, calls: 0 }
    }

    fn step(&mut self, event: Event) -> Vec<Asked> {
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires every deadline due at `now`.
    fn fire(&mut self, now: Time) -> Vec<Asked> {
        self.env.now = now;
        let mut asked = Vec::new();
        while self.domain.is_due(now) {
            self.meter.start();
            fire(&mut self.domain, &self.env, &mut self.out);
            asked.extend(self.drain());
        }
        asked
    }

    fn drain(&mut self) -> Vec<Asked> {
        let measured = self.meter.end();
        let mut asked = Vec::new();
        while let Some(request) = self.out.pop() {
            asked.push(match request {
                Request::Rendered { .. } | Request::Failed { .. } | Request::Refused { .. } => Asked::Answer,
                Request::Room => Asked::Room,
                Request::Read { owner, .. } => Asked::Read(owner),
            });
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        asked
    }

    /// Asks for a brief of `sections` sections, each naming as many items as
    /// a source may.
    fn render(&mut self, sections: u32, required: bool) -> Vec<Asked> {
        self.calls += 1;
        let reply_to = ReplyTo::new(Token::new(self.calls));
        let items: Box<[Item]> = (0..self.env.limits.items).map(|n| Item { repository: 0, number: n.into() }).collect();
        let wanted = (0..sections).map(|_| Wanted { source: Source::Dependencies(items.clone()), required }).collect();
        self.step(Event::Render { reply_to, sections: wanted })
    }

    /// Answers the read `owner` with as much as a read may bring.
    fn full(&mut self, owner: Token) -> Vec<Asked> {
        self.step(Event::Read { owner, read: content(&self.env.limits) })
    }
}

fn reads(asked: &[Asked]) -> Vec<Token> {
    asked
        .iter()
        .map(|asked| match asked {
            Asked::Read(owner) => *owner,
            Asked::Answer | Asked::Room => panic!("a read per section: {asked:?}"),
        })
        .collect()
}

/// Content of the limits: as many parts as a read may bring, filling its
/// bytes.
fn content(limits: &Limits) -> Read {
    let parts = usize::try_from(limits.parts).expect("small");
    let bytes = usize::try_from(limits.read_bytes).expect("small");
    let each = bytes / parts;
    let mut content: Vec<Part> = (0..parts).map(|_| Part { bytes: vec![b'x'; each].into(), left: 7 }).collect();
    let last = content.last_mut().expect("a part at least");
    last.bytes = vec![b'y'; bytes - each * (parts - 1)].into();
    Read::Got(content.into())
}

/// Fills the brief to its limits: every brief gathering, each with every
/// section but its last read in full; then each rendered, cut to its
/// budgets, by its last read or its deadline.
fn fill(limits: Limits) {
    let mut brief = Measured::new(limits);
    let mut last = Vec::new();
    for _ in 0..limits.briefs {
        let owners = reads(&brief.render(limits.sections, false));
        let (rest, first) = owners.split_last().expect("a section at least");
        for owner in first {
            assert!(brief.full(*owner).is_empty(), "the brief waits for its last read");
        }
        last.push(*rest);
    }
    assert_eq!(brief.domain.briefs(), limits.briefs);
    // At its fullest, with sections to spare for their last reads, the brief
    // holds a fair share of the bound: it is not loose past use.
    let fullest = brief.meter.held();
    if limits.sections > 2 {
        assert!(fullest.saturating_mul(3) > brief.bound, "{fullest} held of a worst case of {}", brief.bound);
    }
    assert_eq!(brief.render(1, false), [Asked::Answer], "refused as busy");
    // The first briefs rendered by their last read, the last by its
    // deadline.
    let (late, rest) = last.split_last().expect("a brief at least");
    let mut answered = Vec::new();
    for owner in rest {
        answered.extend(brief.full(*owner));
    }
    answered.extend(brief.fire(Time::ZERO.saturating_add(limits.gather)));
    let rooms = answered.iter().filter(|asked| **asked == Asked::Room).count();
    assert_eq!((answered.len(), rooms), (last.len() + 1, 1), "an answer each, and room told once: {answered:?}");
    assert!(brief.full(*late).is_empty(), "a late read is dropped");
    assert_eq!((brief.domain.briefs(), brief.domain.reads()), (0, 0));
}

/// Every entry point's other ends: a brief of no sections, a brief past the
/// limits, a read failing or past the bounds, a required section failing.
fn paths(limits: Limits) {
    let mut brief = Measured::new(limits);
    assert_eq!(brief.render(0, false), [Asked::Answer], "rendered at once");
    assert_eq!(brief.render(limits.sections + 1, false), [Asked::Answer], "refused as oversized");
    let owners = reads(&brief.render(2, true));
    let Read::Got(mut parts) = content(&limits) else { unreachable!("content is got") };
    parts[0].bytes = vec![b'z'; 1].into();
    let mut past: Vec<Part> = parts.into_vec();
    past.push(Part { bytes: Box::new([]), left: 0 });
    let failed = brief.step(Event::Read { owner: owners[0], read: Read::Got(past.into()) });
    assert_eq!(failed, [Asked::Answer], "a required section past the bounds fails the brief");
    assert!(brief.step(Event::Read { owner: owners[1], read: Read::Failed }).is_empty());
    let owners = reads(&brief.render(1, false));
    assert_eq!(brief.step(Event::Read { owner: owners[0], read: Read::Failed }), [Asked::Answer]);
    assert_eq!((brief.domain.briefs(), brief.domain.reads()), (0, 0));
}

#[test]
fn a_brief_full_to_its_limits_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { briefs: 6, sections: 8, brief_bytes: 4096, ..LIMITS });
    fill(Limits { parts: 64, read_bytes: 16_384, items: 32, ..LIMITS });
    fill(Limits { briefs: 1, sections: 1, parts: 1, items: 1, ..LIMITS });
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { sections: 2, parts: 2, ..LIMITS });
}
