//! Memory stays within the worst case (programming-style.md, 6.4), measured by
//! a counting allocator: every item taken in and waiting for an alarm, and
//! every entry point on the way through an item's lifecycle.

use temper_engine_model_work::{
    Acted, Answer, Applied, Class, Due, Event, Failures, Hold, Item, Lifecycle, Limits, Model, Phase, Read, Request,
    Retries, Retry, Then, Wrote, fire, max_out, step, worst_case,
};
use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const RETRY: Retry = Retry { retries: 1, base: Duration::from_secs(1), max: Duration::from_secs(4) };

const LIMITS: Limits = Limits {
    items: 8,
    undelivered: 2,
    retries: Retries { transient: RETRY, permanent: RETRY, run: RETRY, agent: RETRY, lost: RETRY, invalid: RETRY },
    facts: 16,
};

/// What a step asked for, on the stack: the test holds no heap of its own
/// while a step is measured.
type Asked = [Option<Request>; 4];

const NONE: Asked = [None, None, None, None];

/// The hub under `limits`, measured: each step's peak is checked against the
/// worst case.
struct Measured {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    calls: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        // The output queue is its owner's to count: the meter starts after it.
        let out = Queue::with_capacity(max_out(&limits));
        let meter = Meter::new();
        let model = Model::new(&limits, 1);
        Measured { model, env: Env { now: Time::ZERO, limits }, out, meter, bound, calls: 0 }
    }

    fn step(&mut self, event: Event) -> Asked {
        self.meter.start();
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires the alarm due by `now`.
    fn fire(&mut self, now: Time) -> Asked {
        self.env.now = now;
        assert!(self.model.is_due(now), "an alarm is due");
        self.meter.start();
        fire(&mut self.model, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Asked {
        let measured = self.meter.end();
        let mut asked = NONE;
        for slot in &mut asked {
            *slot = self.out.pop();
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.model.reclaim();
        asked
    }

    fn reply(&mut self) -> ReplyTo {
        self.calls += 1;
        ReplyTo::new(Token::new(self.calls))
    }

    fn take(&mut self, item: Item, read: Read) -> Asked {
        let reply_to = self.reply();
        self.step(Event::Take { reply_to, item, read })
    }

    /// Lands every record write in `asked`, and those that follow, answering
    /// what is due with `due`; returns what is left.
    fn written(&mut self, asked: Asked) -> Asked {
        let mut left = NONE;
        let mut at = 0;
        for request in asked.into_iter().flatten() {
            match request {
                Request::Write { owner, .. } => {
                    for more in self.step(Event::Written { owner, wrote: Wrote::Done }).into_iter().flatten() {
                        left[at] = Some(more);
                        at += 1;
                    }
                }
                other @ (Request::Taken { .. }
                | Request::Stopped { .. }
                | Request::Released { .. }
                | Request::Refused { .. }
                | Request::Due { .. }
                | Request::Record { .. }
                | Request::Apply { .. }
                | Request::Act { .. }
                | Request::Start { .. }
                | Request::Adopt { .. }
                | Request::Cancel { .. }
                | Request::Relay { .. }
                | Request::Keep { .. }
                | Request::Acknowledge { .. }
                | Request::Stale { .. }
                | Request::Left { .. }) => {
                    left[at] = Some(other);
                    at += 1;
                }
            }
        }
        left
    }
}

fn item(number: u64) -> Item {
    Item { repository: 0, number }
}

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

/// The owner of the one request in `asked` that names one.
fn owner(asked: &Asked) -> Token {
    for request in asked.iter().flatten() {
        match request {
            Request::Due { owner, .. }
            | Request::Write { owner, .. }
            | Request::Record { owner, .. }
            | Request::Apply { owner, .. }
            | Request::Act { owner, .. } => return *owner,
            Request::Taken { .. }
            | Request::Stopped { .. }
            | Request::Released { .. }
            | Request::Refused { .. }
            | Request::Start { .. }
            | Request::Adopt { .. }
            | Request::Cancel { .. }
            | Request::Relay { .. }
            | Request::Keep { .. }
            | Request::Acknowledge { .. }
            | Request::Stale { .. }
            | Request::Left { .. } => {}
        }
    }
    panic!("a request names its owner: {asked:?}")
}

/// Fills the hub: every item taken in, new, its record written, and waiting
/// for an alarm of its own; one more refused at the entrance.
fn fill(limits: Limits) {
    let mut hub = Measured::new(limits);
    for number in 0..u64::from(limits.items) {
        let asked = hub.take(item(number), Read::New);
        let asked = hub.written(asked);
        let owner = owner(&asked);
        let until = Some(at(number + 1));
        assert!(matches!(hub.step(Event::Decided { owner, due: Due::Nothing { until } }), NONE));
    }
    assert_eq!(hub.model.items(), limits.items);
    let refused = hub.take(item(u64::from(limits.items)), Read::New);
    assert!(matches!(refused, [Some(Request::Refused { .. }), None, None, None]), "{refused:?}");
    // At its fullest, the hub holds a fair share of the bound: it is not
    // loose past use.
    let fullest = hub.meter.held();
    assert!(fullest.saturating_mul(3) > hub.bound, "{fullest} held of a worst case of {}", hub.bound);
    for number in 0..u64::from(limits.items) {
        assert!(matches!(hub.fire(at(number + 1)), [Some(Request::Due { .. }), None, None, None]), "it asks");
    }
}

/// Every entry point on the way through an item's lifecycle: taken in from a
/// record in every phase, a claim and its run, each answer, each
/// application, actions, holds, releases, stops, alarms.
fn paths(limits: Limits) {
    let mut hub = Measured::new(limits);
    let failures = Failures::NONE;
    let read = |phase| Read::Record(Lifecycle { phase, attempts: 1, failures });
    let waiting = hub.take(item(0), read(Phase::Waiting));
    hub.take(item(1), read(Phase::Parked));
    hub.take(item(2), read(Phase::Retrying(Class::Run)));
    hub.take(item(3), read(Phase::Claimed));
    let applying = hub.take(item(4), read(Phase::Applying { outcome: 9 }));
    hub.take(item(5), read(Phase::Held { why: Hold::Stopped, outcome: None }));
    hub.take(item(6), Read::Mangled { attempts: 3 });
    // The one waiting runs: claimed, started, relayed to, answered with an
    // outcome, which is recorded and applied; then it waits for acceptance,
    // is released, and is stale.
    let (the, owner) = (item(0), owner(&waiting));
    let asked = hub.step(Event::Decided { owner, due: Due::Run { run: Token::new(1) } });
    let asked = hub.written(asked);
    assert!(matches!(asked, [Some(Request::Start { .. }), None, None, None]), "{asked:?}");
    hub.step(Event::Placed { item: the, attempt: 2 });
    hub.step(Event::Inbox { item: the, event: Token::new(2), wake: None });
    let answer = Answer::Ended { outcome: Token::new(3) };
    hub.step(Event::Answered { item: the, attempt: 2, answer });
    let asked = hub.step(Event::Recorded { owner, comment: Some(4) });
    let asked = hub.written(asked);
    assert!(matches!(asked, [Some(Request::Acknowledge { .. }), Some(Request::Apply { .. }), None, None]), "{asked:?}");
    let asked = hub.step(Event::Applied { owner, applied: Applied::Accepting });
    hub.written(asked);
    let reply_to = hub.reply();
    let asked = hub.step(Event::Release { reply_to, item: the });
    hub.written(asked);
    let asked = hub.step(Event::Applied { owner, applied: Applied::Stale });
    hub.written(asked);
    // The applying one: made, then an action, then its step done.
    let owner = self::owner(&applying);
    let asked = hub.step(Event::Applied { owner, applied: Applied::Made(Then::Wait) });
    hub.written(asked);
    hub.step(Event::Decided { owner, due: Due::Act { action: Token::new(5) } });
    let asked = hub.step(Event::Acted { owner, acted: Acted::Made });
    hub.written(asked);
    hub.step(Event::Decided { owner, due: Due::Done { action: Token::new(6) } });
    let asked = hub.step(Event::Acted { owner, acted: Acted::Made });
    let asked = hub.written(asked);
    assert!(matches!(asked, [Some(Request::Left { .. }), None, None, None]), "{asked:?}");
    // The retrying one asks what is due once it has backed off, runs, and
    // fails; its stale answers are dropped.
    let asked = hub.fire(at(1));
    let owner = self::owner(&asked);
    let asked = hub.step(Event::Decided { owner, due: Due::Run { run: Token::new(7) } });
    hub.written(asked);
    for answer in [Answer::Failed(Class::Agent), Answer::Lost, Answer::Parked { snapshot: Some(Token::new(8)) }] {
        let asked = hub.step(Event::Answered { item: item(2), attempt: 2, answer });
        hub.written(asked);
    }
    // Refused before anything ran: it pauses, and claims again.
    let asked = hub.step(Event::Answered { item: item(2), attempt: 2, answer: Answer::Refused });
    hub.written(asked);
    // The mangled one learns of an attempt a worker holds.
    hub.step(Event::Listed { item: item(6), attempt: 9 });
    // The claimed one, adopted: inbound events kept until it is placed;
    // stopped, then presumed lost by the fleet, and held.
    hub.step(Event::Undelivered { item: item(3), attempt: 1, event: Token::new(9) });
    hub.step(Event::Placed { item: item(3), attempt: 1 });
    let reply_to = hub.reply();
    hub.step(Event::Stop { reply_to, item: item(3) });
    let asked = hub.step(Event::Answered { item: item(3), attempt: 1, answer: Answer::Lost });
    hub.written(asked);
    assert_eq!(hub.model.items(), 6, "the done one left");
}

#[test]
fn a_hub_full_to_its_limits_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { items: 64, facts: 256, ..LIMITS });
    fill(Limits { items: 1, facts: 1, ..LIMITS });
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { facts: 1, ..LIMITS });
}
