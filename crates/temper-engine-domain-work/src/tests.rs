//! Feed the domain events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::{
    Acted, Answer, Applied, Class, Domain, Due, Event, Fact, Failures, Hold, Item, Lifecycle, Limits, Phase, Read,
    Refusal, Request, Retries, Retry, Then, Wrote, fire, max_out, step, worst_case,
};

const SECOND: Duration = Duration::from_secs(1);

/// Every class retried twice, after a second, doubling up to eight.
const RETRY: Retry = Retry { retries: 2, base: SECOND, max: Duration::from_secs(8) };

const LIMITS: Limits = Limits {
    items: 3,
    retries: Retries {
        transient: RETRY,
        permanent: Retry { retries: 0, ..RETRY },
        run: RETRY,
        agent: RETRY,
        lost: RETRY,
        invalid: Retry { retries: 1, ..RETRY },
    },
    undelivered: 2,
    facts: 64,
};

const ITEM: Item = Item { repository: 0, number: 7 };
const OTHER: Item = Item { repository: 1, number: 7 };

/// The parent's names for what it passes on.
const RUN: Token = Token::new(100);
const ACTION: Token = Token::new(200);
const OUTCOME: Token = Token::new(300);
const SNAPSHOT: Token = Token::new(400);
const EVENT: Token = Token::new(500);
/// The plan's reason to hold, as the parent codes it.
const REASON: u32 = 6;

/// The comment an outcome is posted as.
const COMMENT: u64 = 42;

/// The domain, its environment, and room for one step's output.
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    calls: u64,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness::seeded(limits, 1)
    }

    fn seeded(limits: Limits, seed: u64) -> Harness {
        let out = Queue::with_capacity(max_out(&limits));
        Harness { domain: Domain::new(&limits, seed), env: Env { now: Time::ZERO, limits }, out, calls: 0 }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Moves the clock to `now` and fires what is due, returning what it
    /// emitted.
    fn fire_at(&mut self, now: Time) -> Box<[Request]> {
        self.env.now = now;
        assert!(self.domain.is_due(now), "an alarm is due at {now:?}");
        fire(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Box<[Request]> {
        let most = max_out(&self.env.limits);
        let mut requests = List::with_capacity(most);
        for _ in 0..most {
            let Some(request) = self.out.pop() else { break };
            requests.push(request).expect("room for max_out");
        }
        assert!(self.out.is_empty(), "a step emits at most max_out");
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        requests.into_boxed()
    }

    /// A call's right to reply, and the token that names it.
    fn reply(&mut self) -> (ReplyTo, Token) {
        self.calls = self.calls.checked_add(1).unwrap();
        (ReplyTo::new(Token::new(self.calls)), Token::new(self.calls))
    }

    /// Takes `item` in from `read`, expecting it taken, and returns what it
    /// asks next.
    fn take(&mut self, item: Item, read: Read) -> Box<[Request]> {
        let (reply_to, to) = self.reply();
        let asked = self.step(Event::Take { reply_to, item, read });
        let mut rest = List::with_capacity(max_out(&self.env.limits));
        let mut taken = false;
        for request in asked {
            if taken {
                rest.push(request).unwrap();
                continue;
            }
            assert_eq!(request, Request::Taken { to: ReplyTo::new(to) }, "the item is taken in first");
            taken = true;
        }
        assert!(taken, "the item is taken in");
        rest.into_boxed()
    }

    fn refused_take(&mut self, item: Item, read: Read) -> Refusal {
        let (reply_to, to) = self.reply();
        let asked = self.step(Event::Take { reply_to, item, read });
        let [Request::Refused { to: refused, refusal }] = &*asked else { panic!("the item is refused: {asked:?}") };
        assert_eq!(*refused, ReplyTo::new(to));
        *refusal
    }

    fn stop(&mut self, item: Item) -> Box<[Request]> {
        let (reply_to, _) = self.reply();
        self.step(Event::Stop { reply_to, item })
    }

    fn release(&mut self, item: Item) -> Box<[Request]> {
        let (reply_to, _) = self.reply();
        self.step(Event::Release { reply_to, item })
    }

    /// Takes `item` in, new, and lands its first record: it asks what is due.
    /// Returns its owner token.
    fn waiting(&mut self, item: Item) -> Token {
        let asked = self.take(item, Read::New);
        let owner = written_as(&asked, Phase::Waiting, 0);
        let asked = self.step(Event::Written { owner, wrote: Wrote::Done });
        assert_eq!(&*asked, [Request::Due { owner, item }]);
        owner
    }

    /// Takes `item` in and claims a run for it, landing the claim: the run
    /// starts. Returns the owner token and the attempt.
    fn running(&mut self, item: Item) -> (Token, u64) {
        let owner = self.waiting(item);
        self.claim(owner, item, 1)
    }

    /// Decides a run for the asking `owner`, and lands its claim of
    /// `attempt`.
    fn claim(&mut self, owner: Token, item: Item, attempt: u64) -> (Token, u64) {
        let asked = self.step(Event::Decided { owner, due: Due::Run { run: RUN } });
        assert_eq!(written_as(&asked, Phase::Claimed, attempt), owner);
        let asked = self.step(Event::Written { owner, wrote: Wrote::Done });
        assert_eq!(&*asked, [Request::Start { item, attempt, run: RUN }], "the run starts once its claim is written");
        assert!(self.step(Event::Placed { item, attempt }).is_empty());
        (owner, attempt)
    }

    /// Answers the running `item`'s attempt with an outcome and posts it:
    /// the record says it is being applied, and once it is written the
    /// answer is acknowledged and the outcome applied.
    fn ended(&mut self, owner: Token, item: Item, attempt: u64) {
        let answer = Answer::Ended { outcome: OUTCOME };
        let asked = self.step(Event::Answered { item, attempt, answer });
        assert_eq!(&*asked, [Request::Record { owner, item, attempt, outcome: OUTCOME }]);
        let asked = self.step(Event::Recorded { owner, comment: Some(COMMENT) });
        assert_eq!(written_as(&asked, Phase::Applying { outcome: COMMENT }, attempt), owner);
        let asked = self.step(Event::Written { owner, wrote: Wrote::Done });
        assert_eq!(
            &*asked,
            [Request::Acknowledge { item, attempt }, Request::Apply { owner, item, attempt, outcome: COMMENT }],
            "acknowledged once the record says it is applying"
        );
    }

    fn facts(&mut self) -> Box<[Fact]> {
        let mut facts = List::with_capacity(LIMITS.facts);
        for _ in 0..LIMITS.facts {
            let Some(fact) = self.domain.pop_fact() else { break };
            facts.push(fact).unwrap();
        }
        facts.into_boxed()
    }
}

/// The owner of the one record write in `asked`, checking its phase and
/// attempts.
fn written_as(asked: &[Request], phase: Phase, attempts: u64) -> Token {
    let [Request::Write { owner, lifecycle, .. }] = asked else { panic!("one record write: {asked:?}") };
    assert_eq!(lifecycle.phase, phase, "{asked:?}");
    assert_eq!(lifecycle.attempts, attempts, "{asked:?}");
    *owner
}

/// Held for `why`, keeping no outcome.
fn held(why: Hold) -> Phase {
    Phase::Held { why, outcome: None }
}

fn record(phase: Phase, attempts: u64) -> Read {
    Read::Record(Lifecycle { phase, attempts, failures: Failures::NONE })
}

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

// Taking work in.

#[test]
fn a_new_item_has_its_first_record_written_then_asks_what_is_due() {
    let mut h = Harness::new(LIMITS);
    let asked = h.take(ITEM, Read::New);
    let [Request::Write { owner, item, lifecycle }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(*item, ITEM);
    assert_eq!(*lifecycle, Lifecycle { phase: Phase::Waiting, attempts: 0, failures: Failures::NONE });
    let owner = *owner;
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
    assert_eq!(&*h.facts(), [Fact::Taken { item: ITEM }]);
}

#[test]
fn the_entrance_refuses_an_item_past_the_working_set_and_nothing_taken_in_is_dropped() {
    let mut h = Harness::new(Limits { items: 1, ..LIMITS });
    let owner = h.waiting(ITEM);
    assert_eq!(h.refused_take(OTHER, Read::New), Refusal::Full);
    assert_eq!(h.domain.items(), 1, "the item taken in stays");
    // Once the first is done, the other is taken in.
    assert_eq!(
        &*h.step(Event::Decided { owner, due: Due::Done { action: ACTION } }),
        [Request::Act { owner, item: ITEM, action: ACTION }]
    );
    let asked = h.step(Event::Acted { owner, acted: Acted::Made });
    assert_eq!(written_as(&asked, Phase::Done, 0), owner);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Left { item: ITEM }]);
    assert_eq!(h.domain.items(), 0, "reclaimed");
    h.waiting(OTHER);
}

#[test]
fn an_item_taken_in_twice_or_done_is_refused() {
    let mut h = Harness::new(LIMITS);
    h.waiting(ITEM);
    assert_eq!(h.refused_take(ITEM, Read::New), Refusal::Taken);
    assert_eq!(h.refused_take(OTHER, record(Phase::Done, 3)), Refusal::Done);
    assert_eq!(h.domain.items(), 1);
}

// Restarts: an item read in every phase.

#[test]
fn an_item_read_waiting_or_parked_asks_what_is_due() {
    for phase in [Phase::Waiting, Phase::Parked] {
        let mut h = Harness::new(LIMITS);
        let asked = h.take(ITEM, record(phase, 4));
        let [Request::Due { owner, item: ITEM }] = &*asked else { panic!("{asked:?}") };
        // Its next claim follows the attempts its record counted.
        h.claim(*owner, ITEM, 5);
    }
}

#[test]
fn an_item_read_retrying_backs_off_again_from_its_failures() {
    let mut h = Harness::new(LIMITS);
    let failures = Failures::NONE.and(Class::Run).and(Class::Run);
    let read = Read::Record(Lifecycle { phase: Phase::Retrying(Class::Run), attempts: 2, failures });
    assert!(h.take(ITEM, read).is_empty());
    // Two failures: the second backoff, two seconds, half of it drawn.
    let until = h.domain.next_deadline().expect("it backs off");
    assert!(until >= at(1) && until <= at(2), "{until:?}");
    let asked = h.fire_at(until);
    let [Request::Due { owner, .. }] = &*asked else { panic!("{asked:?}") };
    h.claim(*owner, ITEM, 3);
}

#[test]
fn an_item_read_claimed_adopts_its_run_and_arms_no_grace_of_its_own() {
    let mut h = Harness::new(LIMITS);
    assert_eq!(&*h.take(ITEM, record(Phase::Claimed, 3)), [Request::Adopt { item: ITEM, attempt: 3 }]);
    assert_eq!(h.domain.next_deadline(), None, "the fleet's grace runs the race");
    assert!(h.step(Event::Placed { item: ITEM, attempt: 3 }).is_empty());
    // Its answer is the attempt in flight's.
    let answer = Answer::Ended { outcome: OUTCOME };
    let asked = h.step(Event::Answered { item: ITEM, attempt: 3, answer });
    let [Request::Record { attempt: 3, outcome: OUTCOME, .. }] = &*asked else { panic!("{asked:?}") };
}

#[test]
fn an_item_read_claimed_takes_the_answer_its_worker_kept() {
    let mut h = Harness::new(LIMITS);
    assert_eq!(&*h.take(ITEM, record(Phase::Claimed, 3)), [Request::Adopt { item: ITEM, attempt: 3 }]);
    let asked = h.step(Event::Answered { item: ITEM, attempt: 3, answer: Answer::Parked { snapshot: None } });
    let [Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Parked);
}

#[test]
fn an_item_read_claimed_whose_worker_never_comes_back_is_retried_with_the_next_attempt() {
    let mut h = Harness::new(LIMITS);
    assert_eq!(&*h.take(ITEM, record(Phase::Claimed, 3)), [Request::Adopt { item: ITEM, attempt: 3 }]);
    let asked = h.step(Event::Answered { item: ITEM, attempt: 3, answer: Answer::Lost });
    let [Request::Write { owner, lifecycle, .. }] = &*asked else { panic!("presumed lost: {asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Retrying(Class::Lost));
    assert_eq!(lifecycle.failures.lost, 1);
    let owner = *owner;
    assert!(h.step(Event::Written { owner, wrote: Wrote::Done }).is_empty(), "nothing to acknowledge");
    let until = h.domain.next_deadline().expect("backing off");
    let asked = h.fire_at(until);
    assert_eq!(&*asked, [Request::Due { owner, item: ITEM }]);
    h.claim(owner, ITEM, 4);
    // The lost attempt is the fleet's to fence: anything of it that reaches
    // the hub is stale.
    assert!(h.step(Event::Placed { item: ITEM, attempt: 3 }).is_empty());
    let late = Event::Answered { item: ITEM, attempt: 3, answer: Answer::Failed(Class::Transient) };
    assert_eq!(&*h.step(late), [Request::Stale { item: ITEM, attempt: 3 }]);
}

#[test]
fn an_item_read_applying_applies_its_outcome_again() {
    let mut h = Harness::new(LIMITS);
    let asked = h.take(ITEM, record(Phase::Applying { outcome: COMMENT }, 2));
    let [Request::Apply { owner, item: ITEM, attempt: 2, outcome: COMMENT }] = &*asked else { panic!("{asked:?}") };
    let owner = *owner;
    // The worker sends its answer again: it was acknowledged already, or is
    // now.
    let again = Event::Answered { item: ITEM, attempt: 2, answer: Answer::Ended { outcome: OUTCOME } };
    assert_eq!(&*h.step(again), [Request::Stale { item: ITEM, attempt: 2 }]);
    let asked = h.step(Event::Applied { owner, applied: Applied::Made(Then::Wait) });
    assert_eq!(written_as(&asked, Phase::Waiting, 2), owner);
}

#[test]
fn an_item_read_held_waits_for_a_person() {
    let mut h = Harness::new(LIMITS);
    let why = Hold::Failures(Class::Agent);
    assert!(h.take(ITEM, record(held(why), 5)).is_empty());
    let wake = Event::Inbox { item: ITEM, event: EVENT, wake: Some(Time::ZERO) };
    assert!(h.step(wake).is_empty(), "a held item is not due");
    assert_eq!(h.domain.next_deadline(), None);
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { owner, lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(*lifecycle, Lifecycle { phase: Phase::Waiting, attempts: 5, failures: Failures::NONE });
    let owner = *owner;
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
    assert_eq!(
        &*h.facts(),
        [Fact::Taken { item: ITEM }, Fact::Held { item: ITEM, why }, Fact::Released { item: ITEM }]
    );
}

#[test]
fn a_mangled_record_holds_the_item_and_its_attempts_follow_the_fleet() {
    let mut h = Harness::new(LIMITS);
    assert!(h.take(ITEM, Read::Mangled { attempts: 4 }).is_empty());
    // An attempt below the outcomes' is not counted.
    assert!(h.step(Event::Listed { item: ITEM, attempt: 2 }).is_empty());
    // A worker still holds an attempt of it, which no claim adopted:
    // counted.
    assert!(h.step(Event::Listed { item: ITEM, attempt: 6 }).is_empty());
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { owner, lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.attempts, 6);
    let owner = *owner;
    h.step(Event::Written { owner, wrote: Wrote::Done });
    h.claim(owner, ITEM, 7);
}

// Asking what is due.

#[test]
fn nothing_due_waits_for_the_plan_s_time_or_an_inbox_event() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    assert!(h.step(Event::Decided { owner, due: Due::Nothing { until: Some(at(10)) } }).is_empty());
    assert_eq!(h.domain.next_deadline(), Some(at(10)));
    // A later wake changes nothing; an earlier one moves the alarm.
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: Some(at(20)) }).is_empty());
    assert_eq!(h.domain.next_deadline(), Some(at(10)));
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: Some(at(5)) }).is_empty());
    assert_eq!(h.domain.next_deadline(), Some(at(5)));
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: None }).is_empty(), "it does not wake it");
    assert_eq!(&*h.fire_at(at(5)), [Request::Due { owner, item: ITEM }]);
    assert_eq!(h.domain.next_deadline(), None, "asking, it waits for no alarm");
    assert!(h.step(Event::Decided { owner, due: Due::Nothing { until: None } }).is_empty());
    let now = Event::Inbox { item: ITEM, event: EVENT, wake: Some(at(5)) };
    assert_eq!(&*h.step(now), [Request::Due { owner, item: ITEM }], "a wake now asks at once");
}

#[test]
fn a_wake_while_asking_is_kept_for_after_the_answer() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: Some(Time::ZERO) }).is_empty());
    assert!(h.step(Event::Decided { owner, due: Due::Nothing { until: Some(at(10)) } }).is_empty());
    assert_eq!(h.domain.next_deadline(), Some(Time::ZERO), "it asks again in this iteration");
    assert_eq!(&*h.fire_at(Time::ZERO), [Request::Due { owner, item: ITEM }]);
}

#[test]
fn a_hold_the_plan_decides_is_written_then_held() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    let asked = h.step(Event::Decided { owner, due: Due::Hold { reason: REASON } });
    assert_eq!(written_as(&asked, held(Hold::Plan { reason: REASON }), 0), owner);
    assert!(h.step(Event::Written { owner, wrote: Wrote::Done }).is_empty());
    assert_eq!(&*h.stop(ITEM), [Request::Refused { to: ReplyTo::new(Token::new(h.calls)), refusal: Refusal::Idle }]);
}

// Claims and runs.

#[test]
fn a_run_starts_only_once_its_claim_is_written() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    let asked = h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
    assert_eq!(written_as(&asked, Phase::Claimed, 1), owner);
    // Inbox events before it starts are for its brief: nothing is relayed.
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: Some(Time::ZERO) }).is_empty());
    assert_eq!(
        &*h.step(Event::Written { owner, wrote: Wrote::Done }),
        [Request::Start { item: ITEM, attempt: 1, run: RUN }]
    );
    assert_eq!(
        &*h.step(Event::Inbox { item: ITEM, event: EVENT, wake: None }),
        [Request::Relay { item: ITEM, attempt: 1, event: EVENT }]
    );
}

#[test]
fn a_claim_that_cannot_be_written_holds_the_item_and_starts_nothing() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
    assert!(h.step(Event::Written { owner, wrote: Wrote::Failed }).is_empty());
    // Released, its next claim is past the one that may have landed.
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.attempts, 1);
    h.step(Event::Written { owner, wrote: Wrote::Done });
    h.claim(owner, ITEM, 2);
}

#[test]
fn an_outcome_is_recorded_then_applied_and_the_record_s_update_is_the_commit_point() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Made(Then::Wait) });
    assert_eq!(written_as(&asked, Phase::Waiting, 1), owner);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
    assert_eq!(
        &*h.facts(),
        [
            Fact::Taken { item: ITEM },
            Fact::Claimed { item: ITEM, attempt: 1 },
            Fact::Placed { item: ITEM, attempt: 1 },
            Fact::Ended { item: ITEM, attempt: 1 },
            Fact::Applied { item: ITEM },
        ]
    );
}

#[test]
fn an_outcome_that_holds_its_item_is_committed_held() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Made(Then::Hold { reason: REASON }) });
    assert_eq!(written_as(&asked, held(Hold::Plan { reason: REASON }), 1), owner);
    assert!(h.step(Event::Written { owner, wrote: Wrote::Done }).is_empty());
}

#[test]
fn a_stale_outcome_writes_nothing_of_it_and_the_item_asks_again() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Stale });
    assert_eq!(written_as(&asked, Phase::Waiting, 1), owner);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
}

#[test]
fn an_invalid_outcome_fails_its_run() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Invalid });
    assert_eq!(written_as(&asked, Phase::Retrying(Class::Invalid), 1), owner);
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let until = h.domain.next_deadline().unwrap();
    h.fire_at(until);
    h.claim(owner, ITEM, 2);
    h.ended(owner, ITEM, 2);
    // One retry is all an invalid outcome gets.
    let asked = h.step(Event::Applied { owner, applied: Applied::Invalid });
    assert_eq!(written_as(&asked, held(Hold::Failures(Class::Invalid)), 2), owner);
}

#[test]
fn an_outcome_awaiting_acceptance_is_applied_again_once_released() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Accepting });
    let phase = Phase::Held { why: Hold::Acceptance, outcome: Some(COMMENT) };
    assert_eq!(written_as(&asked, phase, 1), owner);
    assert!(h.step(Event::Written { owner, wrote: Wrote::Done }).is_empty());
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Applying { outcome: COMMENT });
    assert_eq!(
        &*h.step(Event::Written { owner, wrote: Wrote::Done }),
        [Request::Apply { owner, item: ITEM, attempt: 1, outcome: COMMENT }]
    );
}

#[test]
fn an_outcome_whose_writes_fail_or_cannot_be_posted_holds_its_item() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Failed });
    let phase = Phase::Held { why: Hold::Writes, outcome: Some(COMMENT) };
    assert_eq!(written_as(&asked, phase, 1), owner, "the outcome is kept, its writes not all made");
    h.step(Event::Written { owner, wrote: Wrote::Done });
    // Released, it is applied again: what was made is found.
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Applying { outcome: COMMENT });
    assert_eq!(
        &*h.step(Event::Written { owner, wrote: Wrote::Done }),
        [Request::Apply { owner, item: ITEM, attempt: 1, outcome: COMMENT }]
    );

    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Ended { outcome: OUTCOME } });
    let asked = h.step(Event::Recorded { owner, comment: None });
    assert_eq!(written_as(&asked, held(Hold::Writes), 1), owner, "nothing posted, nothing kept");
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Acknowledge { item: ITEM, attempt }]);
}

#[test]
fn a_record_that_cannot_be_written_holds_the_item_and_its_answer_until_a_write_lands() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Ended { outcome: OUTCOME } });
    h.step(Event::Recorded { owner, comment: Some(COMMENT) });
    assert!(h.step(Event::Written { owner, wrote: Wrote::Failed }).is_empty(), "not on the forge: not acknowledged");
    let held = Fact::Held { item: ITEM, why: Hold::Record };
    assert!(h.facts().contains(&held));
    assert!(h.step(outcome_of(attempt)).is_empty(), "a copy is never stale: its worker keeps it");
    // The hold keeps the outcome and the answer: released, the record says
    // it applies, the answer is acknowledged, and the outcome applied.
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Applying { outcome: COMMENT });
    assert_eq!(
        &*h.step(Event::Written { owner, wrote: Wrote::Done }),
        [Request::Acknowledge { item: ITEM, attempt }, Request::Apply { owner, item: ITEM, attempt, outcome: COMMENT }]
    );
}

#[test]
fn a_commit_that_cannot_be_written_holds_the_outcome_to_apply_again() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    h.step(Event::Applied { owner, applied: Applied::Made(Then::Wait) });
    assert!(h.step(Event::Written { owner, wrote: Wrote::Failed }).is_empty());
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Applying { outcome: COMMENT }, "its writes are found, then committed");
}

#[test]
fn a_parked_run_keeps_its_snapshot_and_its_item_waits_for_the_next_wake() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.running(ITEM);
    h.step(Event::Answered { item: ITEM, attempt: 1, answer: Answer::Failed(Class::Agent) });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let until = h.domain.next_deadline().unwrap();
    h.fire_at(until);
    h.claim(owner, ITEM, 2);
    let asked = h.step(Event::Answered { item: ITEM, attempt: 2, answer: Answer::Parked { snapshot: Some(SNAPSHOT) } });
    let [Request::Keep { item: ITEM, attempt: 2, snapshot: SNAPSHOT }, Request::Write { lifecycle, .. }] = &*asked
    else {
        panic!("{asked:?}")
    };
    assert_eq!(lifecycle.phase, Phase::Parked);
    assert_eq!(lifecycle.failures, Failures::NONE, "parking forgives earlier failures");
    assert_eq!(
        &*h.step(Event::Written { owner, wrote: Wrote::Done }),
        [Request::Acknowledge { item: ITEM, attempt: 2 }, Request::Due { owner, item: ITEM }]
    );
    // Parked, it waits for its next wake.
    assert!(h.step(Event::Decided { owner, due: Due::Nothing { until: None } }).is_empty());
    let wake = Event::Inbox { item: ITEM, event: EVENT, wake: Some(h.env.now) };
    assert_eq!(&*h.step(wake), [Request::Due { owner, item: ITEM }]);
}

// Failures, by class.

#[test]
fn every_failure_class_is_retried_as_often_as_it_allows_then_held() {
    for class in [Class::Transient, Class::Permanent, Class::Run, Class::Agent, Class::Lost] {
        let mut h = Harness::new(LIMITS);
        let (owner, _) = h.running(ITEM);
        let retries = LIMITS.retries.of(class).retries;
        for attempt in 1..=u64::from(retries) {
            let asked = h.step(Event::Answered { item: ITEM, attempt, answer: failure(class) });
            assert_eq!(written_as(&asked, Phase::Retrying(class), attempt), owner);
            let asked = h.step(Event::Written { owner, wrote: Wrote::Done });
            match class {
                Class::Lost => assert!(asked.is_empty(), "nothing to acknowledge for a lost run"),
                Class::Transient | Class::Permanent | Class::Run | Class::Agent | Class::Invalid => {
                    assert_eq!(&*asked, [Request::Acknowledge { item: ITEM, attempt }]);
                }
            }
            let until = h.domain.next_deadline().expect("backing off");
            assert_eq!(&*h.fire_at(until), [Request::Due { owner, item: ITEM }]);
            h.claim(owner, ITEM, attempt + 1);
        }
        let attempt = u64::from(retries) + 1;
        let asked = h.step(Event::Answered { item: ITEM, attempt, answer: failure(class) });
        assert_eq!(written_as(&asked, held(Hold::Failures(class)), attempt), owner, "{class:?}");
        h.step(Event::Written { owner, wrote: Wrote::Done });
        assert_eq!(h.domain.next_deadline(), None, "a held item is never due");
        // A release forgives them.
        let asked = h.release(ITEM);
        let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
        assert_eq!(lifecycle.failures, Failures::NONE);
    }
}

/// A run's answer that fails it in `class`.
fn failure(class: Class) -> Answer {
    match class {
        Class::Lost => Answer::Lost,
        Class::Transient | Class::Permanent | Class::Run | Class::Agent | Class::Invalid => Answer::Failed(class),
    }
}

#[test]
fn backoff_doubles_with_each_failure_up_to_its_cap_with_half_of_it_drawn() {
    let limits = Limits { retries: Retries { run: Retry { retries: 10, ..RETRY }, ..LIMITS.retries }, ..LIMITS };
    let mut h = Harness::new(limits);
    let (owner, _) = h.running(ITEM);
    let mut ceiling = SECOND;
    for attempt in 1..=6 {
        h.env.now = at(attempt * 100);
        h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Failed(Class::Run) });
        h.step(Event::Written { owner, wrote: Wrote::Done });
        let wait = h.domain.next_deadline().unwrap().saturating_since(h.env.now);
        let half = Duration::from_nanos(ceiling.as_nanos() / 2);
        assert!(wait >= half && wait <= ceiling, "attempt {attempt}: {wait:?} within {half:?}..={ceiling:?}");
        ceiling = ceiling.saturating_mul(2).min(RETRY.max);
        let until = h.domain.next_deadline().unwrap();
        h.fire_at(until);
        h.claim(owner, ITEM, attempt + 1);
    }
}

#[test]
fn backoff_jitter_is_drawn_from_the_seed() {
    let mut waits = List::with_capacity(8);
    for seed in 0..8 {
        let mut h = Harness::seeded(LIMITS, seed);
        let (owner, attempt) = h.running(ITEM);
        h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Failed(Class::Agent) });
        h.step(Event::Written { owner, wrote: Wrote::Done });
        waits.push(h.domain.next_deadline().unwrap()).unwrap();
    }
    let first = waits.as_slice()[0];
    let mut differ = false;
    for wait in &waits {
        differ |= *wait != first;
    }
    assert!(differ, "seeds draw different jitter: {:?}", waits.as_slice());
    // The same seed draws the same.
    let mut h = Harness::seeded(LIMITS, 3);
    let (owner, attempt) = h.running(ITEM);
    h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Failed(Class::Agent) });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    assert_eq!(h.domain.next_deadline(), Some(waits.as_slice()[3]));
}

// Engine actions.

#[test]
fn an_engine_action_is_made_then_committed() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    let asked = h.step(Event::Decided { owner, due: Due::Act { action: ACTION } });
    assert_eq!(&*asked, [Request::Act { owner, item: ITEM, action: ACTION }]);
    let asked = h.step(Event::Acted { owner, acted: Acted::Made });
    assert_eq!(written_as(&asked, Phase::Waiting, 0), owner);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
}

#[test]
fn a_stale_engine_action_asks_again_at_once() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    h.step(Event::Decided { owner, due: Due::Act { action: ACTION } });
    assert_eq!(&*h.step(Event::Acted { owner, acted: Acted::Stale }), [Request::Due { owner, item: ITEM }]);
}

#[test]
fn an_engine_action_awaiting_acceptance_or_failing_holds_its_item() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    h.step(Event::Decided { owner, due: Due::Act { action: ACTION } });
    let asked = h.step(Event::Acted { owner, acted: Acted::Accepting });
    assert_eq!(written_as(&asked, held(Hold::Acceptance), 0), owner);
    h.step(Event::Written { owner, wrote: Wrote::Done });
    // Released, it asks what is due: the action is decided again.
    let asked = h.release(ITEM);
    let [Request::Released { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, Phase::Waiting);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
    h.step(Event::Decided { owner, due: Due::Done { action: ACTION } });
    let asked = h.step(Event::Acted { owner, acted: Acted::Failed });
    assert_eq!(written_as(&asked, held(Hold::Writes), 0), owner);
}

// People.

#[test]
fn a_stop_cancels_the_run_once_and_holds_the_item_when_it_answers() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    let asked = h.stop(ITEM);
    let [Request::Stopped { .. }, Request::Cancel { item: ITEM, attempt: 1 }] = &*asked else { panic!("{asked:?}") };
    let asked = h.stop(ITEM);
    let [Request::Stopped { .. }] = &*asked else { panic!("cancelled once: {asked:?}") };
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: None }).is_empty(), "no relay to a stopped run");
    let asked = h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Failed(Class::Run) });
    assert_eq!(written_as(&asked, held(Hold::Stopped), 1), owner);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Acknowledge { item: ITEM, attempt }]);
}

#[test]
fn a_stop_while_the_claim_is_written_starts_nothing() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
    let asked = h.stop(ITEM);
    let [Request::Stopped { .. }] = &*asked else { panic!("{asked:?}") };
    let asked = h.step(Event::Written { owner, wrote: Wrote::Done });
    assert_eq!(written_as(&asked, held(Hold::Stopped), 1), owner);
}

#[test]
fn stops_and_releases_are_refused_where_they_mean_nothing() {
    let mut h = Harness::new(LIMITS);
    let asked = h.stop(ITEM);
    let [Request::Refused { refusal: Refusal::Unknown, .. }] = &*asked else { panic!("{asked:?}") };
    let asked = h.release(ITEM);
    let [Request::Refused { refusal: Refusal::Unknown, .. }] = &*asked else { panic!("{asked:?}") };
    h.waiting(ITEM);
    let asked = h.stop(ITEM);
    let [Request::Refused { refusal: Refusal::Idle, .. }] = &*asked else { panic!("{asked:?}") };
    let asked = h.release(ITEM);
    let [Request::Refused { refusal: Refusal::Unheld, .. }] = &*asked else { panic!("{asked:?}") };
}

// Races.

#[test]
fn a_run_s_outcome_crossing_a_cancel_is_still_applied_then_the_item_is_held() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.stop(ITEM);
    h.ended(owner, ITEM, attempt);
    let asked = h.step(Event::Applied { owner, applied: Applied::Made(Then::Wait) });
    assert_eq!(written_as(&asked, held(Hold::Stopped), 1), owner);
}

#[test]
fn a_parked_run_crossing_a_cancel_keeps_its_snapshot_and_is_held() {
    let mut h = Harness::new(LIMITS);
    let (_, attempt) = h.running(ITEM);
    h.stop(ITEM);
    let asked = h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Parked { snapshot: Some(SNAPSHOT) } });
    let [Request::Keep { .. }, Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, held(Hold::Stopped));
}

#[test]
fn a_wake_while_applying_changes_nothing_until_the_commit_and_the_item_asks_after() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.ended(owner, ITEM, attempt);
    let wake = Event::Inbox { item: ITEM, event: EVENT, wake: Some(Time::ZERO) };
    assert!(h.step(wake).is_empty(), "neither relayed nor woken while applying");
    let asked = h.step(Event::Applied { owner, applied: Applied::Made(Then::Wait) });
    written_as(&asked, Phase::Waiting, 1);
    assert_eq!(&*h.step(Event::Written { owner, wrote: Wrote::Done }), [Request::Due { owner, item: ITEM }]);
}

#[test]
fn a_stale_answer_after_a_retry_is_dropped_and_the_retry_s_is_taken() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Lost });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let until = h.domain.next_deadline().unwrap();
    h.fire_at(until);
    h.claim(owner, ITEM, 2);
    // The lost attempt answers after all: too late.
    let late = Event::Answered { item: ITEM, attempt: 1, answer: Answer::Ended { outcome: OUTCOME } };
    assert_eq!(&*h.step(late), [Request::Stale { item: ITEM, attempt: 1 }]);
    h.ended(owner, ITEM, 2);
}

/// The item's attempt `attempt` ended with an outcome.
fn outcome_of(attempt: u64) -> Event {
    Event::Answered { item: ITEM, attempt, answer: Answer::Ended { outcome: OUTCOME } }
}

#[test]
fn a_copy_of_an_answer_being_made_durable_is_never_stale() {
    let mut h = Harness::new(LIMITS);
    let (owner, attempt) = h.running(ITEM);
    h.step(outcome_of(attempt));
    assert!(h.step(outcome_of(attempt)).is_empty(), "posting it: its worker keeps it");
    h.step(Event::Recorded { owner, comment: Some(COMMENT) });
    assert!(h.step(outcome_of(attempt)).is_empty(), "writing that it applies: its worker keeps it");
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let asked = h.step(outcome_of(attempt));
    assert_eq!(&*asked, [Request::Stale { item: ITEM, attempt }], "on the forge: forgotten");
}

#[test]
fn answers_and_runs_of_items_not_taken_in_are_the_parent_s() {
    let mut h = Harness::new(LIMITS);
    let answer = Event::Answered { item: ITEM, attempt: 1, answer: Answer::Failed(Class::Run) };
    assert_eq!(&*h.step(answer), [Request::Stale { item: ITEM, attempt: 1 }]);
    assert!(h.step(Event::Placed { item: ITEM, attempt: 1 }).is_empty());
    assert!(h.step(Event::Undelivered { item: ITEM, attempt: 1, event: EVENT }).is_empty());
    assert!(h.step(Event::Listed { item: ITEM, attempt: 1 }).is_empty());
    assert!(h.step(Event::Inbox { item: ITEM, event: EVENT, wake: Some(Time::ZERO) }).is_empty());
}

#[test]
fn a_stopped_adopted_run_is_cancelled_once_and_held_once_lost() {
    let mut h = Harness::new(LIMITS);
    assert_eq!(&*h.take(ITEM, record(Phase::Claimed, 2)), [Request::Adopt { item: ITEM, attempt: 2 }]);
    let asked = h.stop(ITEM);
    let [Request::Stopped { .. }, Request::Cancel { attempt: 2, .. }] = &*asked else { panic!("{asked:?}") };
    let asked = h.stop(ITEM);
    let [Request::Stopped { .. }] = &*asked else { panic!("cancelled once: {asked:?}") };
    let asked = h.step(Event::Answered { item: ITEM, attempt: 2, answer: Answer::Lost });
    let [Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(lifecycle.phase, held(Hold::Stopped));
}

// Refusals and undelivered events.

#[test]
fn a_run_refused_before_anything_ran_counts_no_failure_and_is_claimed_again_after_a_pause() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    let mut ceiling = RETRY.base;
    for attempt in 1..=4 {
        h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
        h.step(Event::Written { owner, wrote: Wrote::Done });
        h.env.now = at(attempt * 100);
        let asked = h.step(Event::Answered { item: ITEM, attempt, answer: Answer::Refused });
        let [Request::Write { lifecycle, .. }] = &*asked else { panic!("{asked:?}") };
        assert_eq!(*lifecycle, Lifecycle { phase: Phase::Waiting, attempts: attempt, failures: Failures::NONE });
        assert!(h.step(Event::Written { owner, wrote: Wrote::Done }).is_empty(), "nothing to acknowledge");
        let until = h.domain.next_deadline().expect("it pauses");
        let wait = until.saturating_since(h.env.now);
        let half = Duration::from_nanos(ceiling.as_nanos() / 2);
        assert!(wait >= half && wait <= ceiling, "refusal {attempt}: {wait:?} within {half:?}..={ceiling:?}");
        ceiling = ceiling.saturating_mul(2).min(RETRY.max);
        assert_eq!(&*h.fire_at(until), [Request::Due { owner, item: ITEM }]);
    }
    // Once a run is placed, the next refusal pauses as the first did.
    h.claim(owner, ITEM, 5);
    h.env.now = at(1000);
    h.step(Event::Answered { item: ITEM, attempt: 5, answer: Answer::Refused });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let wait = h.domain.next_deadline().unwrap().saturating_since(h.env.now);
    assert!(wait <= RETRY.base, "{wait:?}");
}

#[test]
fn a_refused_run_a_person_stopped_is_held() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    h.stop(ITEM);
    // Cancelled before a worker took it: nothing ran.
    let asked = h.step(Event::Answered { item: ITEM, attempt: 1, answer: Answer::Refused });
    assert_eq!(written_as(&asked, held(Hold::Stopped), 1), owner);
}

#[test]
fn inbound_events_not_delivered_yet_are_kept_and_relayed_again_once_placed() {
    let mut h = Harness::new(LIMITS);
    let owner = h.waiting(ITEM);
    h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let first = Token::new(501);
    let second = Token::new(502);
    for event in [first, second, Token::new(503)] {
        let relayed = h.step(Event::Inbox { item: ITEM, event, wake: None });
        assert_eq!(&*relayed, [Request::Relay { item: ITEM, attempt: 1, event }]);
        assert!(h.step(Event::Undelivered { item: ITEM, attempt: 1, event }).is_empty());
    }
    // As many as the limits keep, in the order they came; the rest stays in
    // the inbox.
    assert_eq!(
        &*h.step(Event::Placed { item: ITEM, attempt: 1 }),
        [
            Request::Relay { item: ITEM, attempt: 1, event: first },
            Request::Relay { item: ITEM, attempt: 1, event: second },
        ]
    );
    assert!(h.step(Event::Placed { item: ITEM, attempt: 1 }).is_empty(), "relayed once");
    // Kept again while its worker is out of contact; gone with the run.
    h.step(Event::Undelivered { item: ITEM, attempt: 1, event: first });
    h.step(Event::Answered { item: ITEM, attempt: 1, answer: Answer::Failed(Class::Run) });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    let until = h.domain.next_deadline().unwrap();
    h.fire_at(until);
    h.step(Event::Decided { owner, due: Due::Run { run: RUN } });
    h.step(Event::Written { owner, wrote: Wrote::Done });
    assert!(h.step(Event::Placed { item: ITEM, attempt: 2 }).is_empty(), "the next run has its own");
    // An event for an attempt not in flight, or a stopped one, is not kept.
    h.step(Event::Undelivered { item: ITEM, attempt: 1, event: first });
    h.stop(ITEM);
    h.step(Event::Undelivered { item: ITEM, attempt: 2, event: second });
    assert!(h.step(Event::Placed { item: ITEM, attempt: 2 }).is_empty());
}

// Bounds.

#[test]
fn facts_beyond_their_room_are_dropped_and_counted() {
    let mut h = Harness::new(Limits { facts: 1, ..LIMITS });
    h.running(ITEM);
    assert_eq!(&*h.facts(), [Fact::Taken { item: ITEM }]);
    assert_eq!(h.domain.facts_lost(), 2, "claimed and running");
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    let more = worst_case(&Limits { items: 4, ..LIMITS }).expect("fits");
    assert!(more > bound, "an item more is more");
    let facts = worst_case(&Limits { facts: 128, ..LIMITS }).expect("fits");
    assert!(facts > bound, "a fact more is more");
    // An item holds no bytes, and nothing in the bound multiplies counts: it
    // fits a u64 under any limits.
    let most = worst_case(&Limits { items: u32::MAX, facts: u32::MAX, ..LIMITS }).expect("fits");
    assert!(most > more);
}
