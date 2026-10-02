//! Memory stays within the worst case (programming-style.md, 6.4), measured by
//! a counting allocator: every run followed, every watcher with a full
//! backlog, the batch full while the store is behind, and every entry point
//! on the way.

use temper_engine_model_views::{
    Capture, Event, Kind, Limits, Model, Phase, Policy, Request, Subject, fire, max_out, step, worst_case,
};
use temper_lib::{Duration, Env, Queue, Time, Token};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    runs: 4,
    watchers: 6,
    backlog: 8,
    report_bytes: 512,
    records: 16,
    batch_bytes: 4096,
    appends: 2,
    flush: Duration::from_secs(1),
    retention: Duration::from_secs(60),
    sweep: Duration::from_secs(10),
    facts: 16,
};

/// A policy that keeps every report whole.
const WHOLE: Policy = Policy {
    text: Capture::Content,
    progress: Capture::Content,
    calls: Capture::Content,
    tools: Capture::Content,
    usage: Capture::Content,
};

/// What a step asked for, without the payload: an answer or an end to a
/// watch, a delivery, or a store operation, by its owner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    Watching,
    Refused,
    Deliver,
    Ended,
    Append(Token),
    Expire(Token),
}

/// The views under `limits`, measured: each step's peak is checked against
/// the worst case, less what it handed out in requests, which their
/// receivers count.
struct Measured {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        let meter = Meter::new();
        let model = Model::new(&limits);
        let out = Queue::with_capacity(max_out(&limits));
        Measured { model, env: Env { now: Time::ZERO, limits }, out, meter, bound }
    }

    fn step(&mut self, event: Event) -> Vec<Asked> {
        self.meter.start();
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires every deadline due at `now`.
    fn fire(&mut self, now: Time) -> Vec<Asked> {
        self.env.now = now;
        let mut asked = Vec::new();
        while self.model.is_due(now) {
            self.meter.start();
            fire(&mut self.model, &self.env, &mut self.out);
            asked.extend(self.drain());
        }
        asked
    }

    fn drain(&mut self) -> Vec<Asked> {
        let measured = self.meter.end();
        let mut asked = Vec::new();
        while let Some(request) = self.out.pop() {
            asked.push(match request {
                Request::Watching { .. } => Asked::Watching,
                Request::Refused { .. } => Asked::Refused,
                Request::Deliver { .. } => Asked::Deliver,
                Request::Ended { .. } => Asked::Ended,
                Request::Append { owner, .. } => Asked::Append(owner),
                Request::Expire { owner, .. } => Asked::Expire(owner),
            });
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.model.reclaim();
        asked
    }

    /// The run `run` reports as much as a report may hold.
    fn report(&mut self, run: u64) -> Vec<Asked> {
        let content = vec![b'x'; usize::try_from(self.env.limits.report_bytes).expect("small")].into();
        self.step(Event::Reported { run: Token::new(run), kind: Kind::Text, content })
    }

    fn watch(&mut self, watcher: u64, subject: Subject) -> Vec<Asked> {
        self.step(Event::Watch { watcher: Token::new(watcher), subject })
    }

    fn delivered(&mut self, watcher: u64) -> Vec<Asked> {
        self.step(Event::Delivered { watcher: Token::new(watcher) })
    }
}

fn appends(asked: &[Asked]) -> Vec<Token> {
    asked
        .iter()
        .filter_map(|asked| match asked {
            Asked::Append(owner) => Some(*owner),
            Asked::Watching | Asked::Refused | Asked::Deliver | Asked::Ended | Asked::Expire(_) => None,
        })
        .collect()
}

/// Fills the views to their limits: every run followed, every watcher of
/// the first with a delivery in flight and a full backlog, the batch full
/// while the store is behind; then lets it all go.
fn fill(limits: Limits) {
    let mut views = Measured::new(limits);
    for run in 0..u64::from(limits.runs) {
        assert!(views.step(Event::Started { run: Token::new(run), item: Token::new(run), policy: WHOLE }).is_empty());
    }
    let watchers: Vec<u64> = (100..100 + u64::from(limits.watchers)).collect();
    for &watcher in &watchers {
        assert_eq!(views.watch(watcher, Subject::Run(Token::new(0))), [Asked::Watching]);
    }
    assert_eq!(views.watch(1, Subject::Board(0)), [Asked::Refused], "refused as busy");
    let mut sent = Vec::new();
    for _ in 0..=limits.backlog {
        sent.extend(appends(&views.report(0)));
    }
    // A report more than the store can take while it is behind.
    for _ in 0..limits.records {
        sent.extend(appends(&views.report(1)));
    }
    assert!(sent.len() <= usize::try_from(limits.appends).expect("small"), "the store is behind");
    // At its fullest, the views hold a fair share of the bound: it is not
    // loose past use.
    let fullest = views.meter.held();
    assert!(fullest.saturating_mul(3) > views.bound, "{fullest} held of a worst case of {}", views.bound);
    // One more report overflows every backlog; then each delivery ends, and
    // what waits goes whole.
    views.report(0);
    for &watcher in &watchers {
        assert_eq!(views.delivered(watcher), [Asked::Deliver]);
    }
    // The run finishes: each watcher ends once its delivery has.
    assert!(views.step(Event::Finished { run: Token::new(0) }).is_empty());
    for &watcher in &watchers {
        assert_eq!(views.delivered(watcher), [Asked::Ended]);
    }
    // The store catches up, and is swept.
    for owner in sent {
        let next = appends(&views.step(Event::Appended { owner, done: true }));
        assert!(next.len() <= 1, "the next batch goes");
        for owner in next {
            views.step(Event::Appended { owner, done: true });
        }
    }
    let flushed = views.fire(Time::ZERO.saturating_add(limits.flush));
    for owner in appends(&flushed) {
        views.step(Event::Appended { owner, done: false });
    }
    let [Asked::Expire(owner)] = views.fire(Time::ZERO.saturating_add(limits.sweep))[..] else { panic!("a sweep") };
    assert!(views.step(Event::Expired { owner, done: true }).is_empty());
    assert_eq!((views.model.watchers(), views.model.ops(), views.model.batched()), (0, 0, 0));
}

/// Every entry point's other ends: a watch of a run not followed, a run
/// past the limits, a report past them or of a run not followed, a phase
/// change, a watch stopped idle or with a delivery in flight, the store
/// failing.
fn paths(limits: Limits) {
    let mut views = Measured::new(limits);
    let run = Token::new(0);
    assert!(views.step(Event::Started { run, item: Token::new(10), policy: WHOLE }).is_empty());
    assert_eq!(views.watch(1, Subject::Run(Token::new(9))), [Asked::Refused], "a run not followed");
    for extra in 1..=u64::from(limits.runs) {
        views.step(Event::Started { run: Token::new(extra), item: Token::new(10), policy: WHOLE });
    }
    let past = vec![b'x'; usize::try_from(limits.report_bytes).expect("small") + 1].into();
    assert!(views.step(Event::Reported { run, kind: Kind::Tool, content: past }).is_empty());
    assert!(views.report(99).is_empty(), "a run not followed");
    assert_eq!(views.watch(2, Subject::Item(Token::new(10))), [Asked::Watching]);
    assert_eq!(views.watch(3, Subject::Board(0)), [Asked::Watching]);
    let phase = Event::Phase { item: Token::new(10), repository: 0, phase: Phase::Held };
    assert_eq!(views.step(phase), [Asked::Deliver, Asked::Deliver]);
    assert!(views.report(0).is_empty(), "it waits for the item's watcher's delivery");
    assert!(views.step(Event::Unwatch { watcher: Token::new(3) }).is_empty());
    assert_eq!(views.delivered(3), [Asked::Ended]);
    assert_eq!(views.delivered(2), [Asked::Deliver]);
    assert!(views.delivered(2).is_empty());
    assert_eq!(views.step(Event::Unwatch { watcher: Token::new(2) }), [Asked::Ended]);
    let [Asked::Append(owner)] = views.fire(Time::ZERO.saturating_add(limits.flush))[..] else {
        panic!("the batch goes")
    };
    assert!(views.step(Event::Appended { owner, done: false }).is_empty());
    let [Asked::Expire(owner)] = views.fire(Time::ZERO.saturating_add(limits.flush).saturating_add(limits.sweep))[..]
    else {
        panic!("a sweep")
    };
    assert!(views.step(Event::Expired { owner, done: false }).is_empty());
}

#[test]
fn views_full_to_their_limits_stay_within_their_worst_case() {
    fill(LIMITS);
    fill(Limits { watchers: 16, backlog: 32, ..LIMITS });
    fill(Limits { report_bytes: 4096, batch_bytes: 16_384, records: 64, ..LIMITS });
    fill(Limits { runs: 1, watchers: 1, backlog: 1, records: 1, appends: 1, ..LIMITS });
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { watchers: 2, backlog: 1, records: 2, ..LIMITS });
}
