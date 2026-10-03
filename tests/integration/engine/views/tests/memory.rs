//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: every run followed and as many turned away, every
//! watch open with a full backlog and as many ended within the iteration, the
//! batch full while the store is behind, and every entry point on the way.

use temper_engine_model_views::{
    Capture, Event, Kind, Limits, Model, Policy, Request, Subject, fire, max_out, step, worst_case,
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
    snapshot_bytes: 1024,
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
        let model = Model::new(&limits, Time::ZERO);
        let out = Queue::with_capacity(max_out(&limits));
        Measured { model, env: Env { now: Time::ZERO, limits }, out, meter, bound }
    }

    /// Steps `event`, and ends the iteration: the reclaim point.
    fn step(&mut self, event: Event) -> Vec<Asked> {
        let asked = self.held(event);
        self.model.reclaim();
        asked
    }

    /// Steps `event` within the iteration: the reclaim point does not
    /// follow.
    fn held(&mut self, event: Event) -> Vec<Asked> {
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
            self.model.reclaim();
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
        asked
    }

    /// The run `run` reports as much as a report may hold.
    fn report(&mut self, run: u64) -> Vec<Asked> {
        self.held(Event::Reported {
            run: Token::new(run),
            kind: Kind::Text,
            content: bytes(self.env.limits.report_bytes),
        })
    }

    /// A watch of `subject` with as large a snapshot as may be.
    fn watch(&mut self, watcher: u64, subject: Subject) -> Vec<Asked> {
        let snapshot = bytes(self.env.limits.snapshot_bytes);
        self.held(Event::Watch { watcher: Token::new(watcher), subject, snapshot })
    }

    fn delivered(&mut self, watcher: u64, done: bool) -> Vec<Asked> {
        self.held(Event::Delivered { watcher: Token::new(watcher), done })
    }
}

/// As many bytes as `len`.
fn bytes(len: u32) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("small")].into()
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

/// Fills the views to their limits, and past them within one iteration:
/// every run followed and as many turned away; every watch of the first run
/// with a delivery in flight and a full backlog, then each ended and as many
/// new ones taken and filled before the reclaim point; the batch full while
/// the store is behind. Then lets it all go.
fn fill(limits: Limits, share: u64) {
    let mut views = Measured::new(limits);
    for run in 0..2 * u64::from(limits.runs) {
        let started =
            Event::Started { run: Token::new(run), attempt: Token::new(1), item: Token::new(run), policy: WHOLE };
        assert!(views.step(started).is_empty());
    }
    let watchers: Vec<u64> = (100..100 + u64::from(limits.watchers)).collect();
    for &watcher in &watchers {
        assert_eq!(views.watch(watcher, Subject::Run(Token::new(0))), [Asked::Watching, Asked::Deliver]);
        assert!(views.delivered(watcher, true).is_empty());
        views.model.reclaim();
    }
    assert_eq!(views.watch(1, Subject::Board(0)), [Asked::Refused], "refused as busy");
    let mut sent = Vec::new();
    for _ in 0..=limits.backlog {
        sent.extend(appends(&views.report(0)));
        views.model.reclaim();
    }
    // A report more than the store can take while it is behind.
    for _ in 0..limits.records {
        sent.extend(appends(&views.report(1)));
        views.model.reclaim();
    }
    assert!(sent.len() <= usize::try_from(limits.appends).expect("small"), "the store is behind");
    // Within one iteration: every watch ends, and as many are taken and
    // filled, the ended ones holding their slots till the reclaim point.
    for &watcher in &watchers {
        assert!(views.held(Event::Unwatch { watcher: Token::new(watcher) }).is_empty());
        assert_eq!(views.delivered(watcher, false), [Asked::Ended]);
    }
    let fresh: Vec<u64> = (200..200 + u64::from(limits.watchers)).collect();
    for &watcher in &fresh {
        assert_eq!(views.watch(watcher, Subject::Run(Token::new(0))), [Asked::Watching, Asked::Deliver]);
    }
    for _ in 0..limits.backlog {
        views.report(0);
    }
    // At its fullest, between steps, the views hold `share` per mille of the
    // bound at least: it is not loose past what a step holds in hand.
    let fullest = views.meter.held();
    let held = fullest.saturating_mul(1000) > views.bound.saturating_mul(share);
    assert!(held, "{fullest} held of a worst case of {}", views.bound);
    views.model.reclaim();
    // One more report overflows every backlog; then each delivery ends, and
    // what waits goes whole.
    views.report(0);
    for &watcher in &fresh {
        assert_eq!(views.delivered(watcher, true), [Asked::Deliver]);
    }
    // The run finishes: each watcher ends once its delivery has.
    assert!(views.step(Event::Finished { run: Token::new(0) }).is_empty());
    for &watcher in &fresh {
        assert_eq!(views.delivered(watcher, false), [Asked::Ended]);
    }
    views.model.reclaim();
    // The store catches up, and is swept.
    for owner in sent {
        let next = appends(&views.step(Event::Appended { owner, done: true }));
        for owner in next {
            views.step(Event::Appended { owner, done: true });
        }
    }
    for owner in appends(&views.fire(Time::ZERO.saturating_add(limits.flush))) {
        views.step(Event::Appended { owner, done: false });
    }
    let [Asked::Expire(owner)] = views.fire(Time::ZERO.saturating_add(limits.sweep))[..] else { panic!("a sweep") };
    assert!(views.step(Event::Expired { owner, done: true }).is_empty());
    assert_eq!((views.model.watchers(), views.model.ops(), views.model.batched()), (0, 0, 0));
}

/// Every entry point's other ends: a watch of a run not followed, turned
/// away, or with a snapshot past the limits; a report past them or of a run
/// not followed; a phase change; a watch stopped idle or with a delivery in
/// flight; a delivery its stream did not take; the store failing, and
/// answering twice.
fn paths(limits: Limits) {
    let mut views = Measured::new(limits);
    let run = Token::new(0);
    let item = Token::new(10);
    assert!(views.step(Event::Started { run, attempt: Token::new(1), item, policy: WHOLE }).is_empty());
    assert_eq!(views.watch(1, Subject::Run(Token::new(9))), [Asked::Refused], "a run not followed");
    for extra in 1..=u64::from(limits.runs) {
        views.step(Event::Started { run: Token::new(extra), attempt: Token::new(1), item, policy: WHOLE });
    }
    let turned = Token::new(u64::from(limits.runs));
    assert_eq!(views.watch(1, Subject::Run(turned)), [Asked::Refused], "a run turned away");
    let large = bytes(limits.snapshot_bytes + 1);
    let refused = views.step(Event::Watch { watcher: Token::new(1), subject: Subject::Board(0), snapshot: large });
    assert_eq!(refused, [Asked::Refused], "a snapshot past the limits");
    let past = bytes(limits.report_bytes + 1);
    assert!(views.step(Event::Reported { run, kind: Kind::Tool, content: past }).is_empty());
    assert!(views.report(99).is_empty(), "a run not followed");
    assert!(views.report(u64::from(limits.runs)).is_empty(), "a run turned away");
    assert_eq!(views.watch(2, Subject::Item(item)), [Asked::Watching, Asked::Deliver]);
    assert_eq!(views.watch(3, Subject::Board(0)), [Asked::Watching, Asked::Deliver]);
    assert!(views.delivered(2, true).is_empty());
    assert!(views.delivered(3, true).is_empty());
    let phase = Event::Phase { item, repository: 0, phase: 3 };
    assert_eq!(views.step(phase), [Asked::Deliver, Asked::Deliver]);
    assert!(views.report(0).is_empty(), "it waits for the item's watcher's delivery");
    assert!(views.step(Event::Unwatch { watcher: Token::new(3) }).is_empty());
    assert_eq!(views.delivered(3, true), [Asked::Ended]);
    assert_eq!(views.delivered(2, false), [Asked::Deliver]);
    assert!(views.delivered(2, true).is_empty());
    assert_eq!(views.step(Event::Unwatch { watcher: Token::new(2) }), [Asked::Ended]);
    views.model.reclaim();
    let [Asked::Append(owner)] = views.fire(Time::ZERO.saturating_add(limits.flush))[..] else {
        panic!("the batch goes")
    };
    assert!(views.held(Event::Appended { owner, done: false }).is_empty());
    assert!(views.step(Event::Appended { owner, done: false }).is_empty(), "twice");
    let [Asked::Expire(owner)] = views.fire(Time::ZERO.saturating_add(limits.sweep))[..] else { panic!("a sweep") };
    assert!(views.step(Event::Expired { owner, done: false }).is_empty());
}

#[test]
fn views_full_to_their_limits_stay_within_their_worst_case() {
    fill(LIMITS, 900);
    fill(Limits { watchers: 16, backlog: 32, ..LIMITS }, 900);
    fill(Limits { report_bytes: 4096, batch_bytes: 16_384, records: 64, ..LIMITS }, 900);
    // Small limits, where what a step holds in hand weighs most.
    fill(Limits { runs: 1, watchers: 1, backlog: 1, records: 1, appends: 1, ..LIMITS }, 600);
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { watchers: 2, backlog: 1, records: 2, ..LIMITS });
}
