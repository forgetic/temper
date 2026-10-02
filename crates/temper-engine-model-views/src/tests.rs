//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, Time, Token};

use crate::{
    Capture, Chunk, Dropped, End, Event, Fact, Kept, Kind, Limits, Lost, Model, Policy, Record, Refusal, Request,
    Subject, fire, max_out, step, worst_case,
};

const LIMITS: Limits = Limits {
    runs: 2,
    watchers: 4,
    backlog: 2,
    report_bytes: 16,
    snapshot_bytes: 8,
    records: 3,
    batch_bytes: 32,
    appends: 2,
    flush: Duration::from_secs(1),
    retention: Duration::from_secs(60),
    sweep: Duration::from_secs(10),
    facts: 64,
};

/// Runs, their attempts, items and watchers, by the parent's tokens.
const RUN: Token = Token::new(1);
const OTHER_RUN: Token = Token::new(2);
const ATTEMPT: Token = Token::new(100);
const ITEM: Token = Token::new(10);
const OTHER_ITEM: Token = Token::new(11);

/// A policy that keeps the same of every kind of report.
const fn policy(capture: Capture) -> Policy {
    Policy { text: capture, progress: capture, calls: capture, tools: capture, usage: capture }
}

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        // Room for what steps within one iteration emit, before the reclaim
        // point drains it.
        let out = Queue::with_capacity(max_out(&limits).saturating_mul(16));
        Harness { model: Model::new(&limits, Time::ZERO), env: Env { now: Time::ZERO, limits }, out }
    }

    /// A harness following `RUN`, for `ITEM`, under `policy`.
    fn following(limits: Limits, policy: Policy) -> Harness {
        let mut h = Harness::new(limits);
        assert!(h.step(Event::Started { run: RUN, attempt: ATTEMPT, item: ITEM, policy }).is_empty());
        assert_eq!(h.facts().as_slice(), [Fact::Followed]);
        h
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Steps `event` within the iteration: no reclaim point follows.
    fn held(&mut self, event: Event) {
        step(&mut self.model, &self.env, event, &mut self.out);
    }

    /// Moves the clock to `secs` seconds and fires every deadline due then.
    fn at(&mut self, secs: u64) -> Box<[Request]> {
        self.at_time(at(secs))
    }

    fn at_time(&mut self, now: Time) -> Box<[Request]> {
        self.env.now = now;
        let mut fired = List::with_capacity(8);
        for _ in 0..4_u32 {
            if !self.model.is_due(self.env.now) {
                break;
            }
            fire(&mut self.model, &self.env, &mut self.out);
            for request in self.drain() {
                fired.push(request).unwrap();
            }
        }
        fired.into_boxed()
    }

    /// What the steps since the last reclaim point emitted; then the reclaim
    /// point.
    fn drain(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(self.out.capacity());
        for _ in 0..self.out.len() {
            requests.push(self.out.pop().unwrap()).unwrap();
        }
        self.model.reclaim();
        requests.into_boxed()
    }

    /// A watch of `subject`, taken, whose snapshot its stream has taken: the
    /// watcher is caught up.
    fn watch(&mut self, watcher: u64, subject: Subject) {
        let token = Token::new(watcher);
        let asked = self.step(Event::Watch { watcher: token, subject, snapshot: Box::from(*b"now") });
        let first = Chunk::Snapshot { at: self.env.now, content: Box::from(*b"now") };
        assert_eq!(&*asked, [Request::Watching { watcher: token }, deliver(watcher, 0, Box::new([first]))]);
        assert!(self.delivered(watcher).is_empty());
    }

    fn report(&mut self, run: Token, content: &[u8]) -> Box<[Request]> {
        self.step(Event::Reported { run, kind: Kind::Text, content: Box::from(content) })
    }

    fn delivered(&mut self, watcher: u64) -> Box<[Request]> {
        self.step(Event::Delivered { watcher: Token::new(watcher), done: true })
    }

    /// The delivery to `watcher` ends untaken by its stream.
    fn failed(&mut self, watcher: u64) -> Box<[Request]> {
        self.step(Event::Delivered { watcher: Token::new(watcher), done: false })
    }

    /// The facts not yet drained.
    fn facts(&mut self) -> List<Fact> {
        let mut facts = List::with_capacity(64);
        for _ in 0..64_u32 {
            let Some(fact) = self.model.pop_fact() else {
                break;
            };
            facts.push(fact).unwrap();
        }
        facts
    }
}

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn text(run: Token, secs: u64, content: &[u8]) -> Chunk {
    Chunk::Report { run, attempt: ATTEMPT, kind: Kind::Text, at: at(secs), content: Box::from(content) }
}

fn deliver(watcher: u64, missed: u64, chunks: Box<[Chunk]>) -> Request {
    Request::Deliver { watcher: Token::new(watcher), missed, chunks }
}

fn ended(watcher: u64, end: End) -> Request {
    Request::Ended { watcher: Token::new(watcher), end }
}

/// The records of an append, and its owner.
fn appended(requests: &[Request]) -> (Token, &[Record]) {
    let [Request::Append { owner, records }] = requests else { panic!("one append: {requests:?}") };
    (*owner, records)
}

/// The owner of an expire, and what it names.
fn expire(requests: &[Request]) -> (Token, Time) {
    let [Request::Expire { owner, before }] = requests else { panic!("one expire: {requests:?}") };
    (*owner, *before)
}

fn record(secs: u64, size: u32, content: Option<Box<[u8]>>) -> Record {
    Record { run: RUN, attempt: ATTEMPT, kind: Kind::Text, at: at(secs), size, content }
}

fn change(item: Token, phase: u32) -> Chunk {
    Chunk::Phase { item, phase, at: Time::ZERO }
}

fn lost(runs: u64, reports: u64, chunks: u64, records: u64) -> Lost {
    Lost { runs, reports, chunks, records, facts: 0 }
}

#[test]
fn a_watch_is_taken_or_refused_at_the_entrance() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Item(OTHER_ITEM));
    h.watch(3, Subject::Board(0));
    let watcher = Token::new(4);
    let unknown = h.step(Event::Watch { watcher, subject: Subject::Run(OTHER_RUN), snapshot: Box::new([]) });
    assert_eq!(&*unknown, [Request::Refused { watcher, refusal: Refusal::Unknown }]);
    let large = h.step(Event::Watch { watcher, subject: Subject::Item(ITEM), snapshot: Box::from(*b"too large") });
    assert_eq!(&*large, [Request::Refused { watcher, refusal: Refusal::Oversized }]);
    h.watch(4, Subject::Item(ITEM));
    let busy = h.step(Event::Watch { watcher: Token::new(5), subject: Subject::Board(1), snapshot: Box::new([]) });
    assert_eq!(&*busy, [Request::Refused { watcher: Token::new(5), refusal: Refusal::Busy }]);
    assert_eq!(h.model.watchers(), 4);
    assert!(h.facts().as_slice().contains(&Fact::Refused { refusal: Refusal::Busy }));
}

#[test]
fn a_watch_begins_with_its_snapshot() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    let watcher = Token::new(1);
    let asked = h.step(Event::Watch { watcher, subject: Subject::Item(ITEM), snapshot: Box::from(*b"held") });
    let first = Chunk::Snapshot { at: Time::ZERO, content: Box::from(*b"held") };
    assert_eq!(&*asked, [Request::Watching { watcher }, deliver(1, 0, Box::new([first]))]);
    assert!(h.report(RUN, b"a").is_empty(), "it waits for the snapshot");
    assert_eq!(&*h.delivered(1), [deliver(1, 0, Box::new([text(RUN, 0, b"a")]))]);
}

#[test]
fn a_watch_ended_frees_its_place_at_once() {
    let mut h = Harness::new(Limits { watchers: 1, ..LIMITS });
    h.watch(1, Subject::Board(0));
    // Within one iteration: the watch ends, and another takes its place
    // before the reclaim point.
    h.held(Event::Unwatch { watcher: Token::new(1) });
    h.held(Event::Watch { watcher: Token::new(2), subject: Subject::Board(0), snapshot: Box::new([]) });
    let first = Chunk::Snapshot { at: Time::ZERO, content: Box::new([]) };
    let watching = Request::Watching { watcher: Token::new(2) };
    assert_eq!(&*h.drain(), [ended(1, End::Unwatched), watching, deliver(2, 0, Box::new([first]))]);
}

#[test]
fn watches_begun_and_ended_within_an_iteration_are_refused_once_no_slot_is_free() {
    let mut h = Harness::new(Limits { watchers: 2, ..LIMITS });
    // Each watch ends at once, and keeps its slot until the reclaim point:
    // twice as many slots as watches.
    for watcher in 1..=4 {
        let token = Token::new(watcher);
        h.held(Event::Watch { watcher: token, subject: Subject::Board(0), snapshot: Box::new([]) });
        h.held(Event::Delivered { watcher: token, done: true });
        h.held(Event::Unwatch { watcher: token });
    }
    let watcher = Token::new(5);
    h.held(Event::Watch { watcher, subject: Subject::Board(0), snapshot: Box::new([]) });
    let asked = h.drain();
    assert_eq!(asked.last(), Some(&Request::Refused { watcher, refusal: Refusal::Busy }), "no slot is free");
    h.watch(5, Subject::Board(0));
}

#[test]
fn a_report_streams_to_the_watchers_of_its_run_and_of_its_item() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Item(ITEM));
    h.watch(3, Subject::Item(OTHER_ITEM));
    h.watch(4, Subject::Board(0));
    h.env.now = at(3);
    let sent = h.report(RUN, b"hello");
    let hello = [deliver(1, 0, Box::new([text(RUN, 3, b"hello")])), deliver(2, 0, Box::new([text(RUN, 3, b"hello")]))];
    assert_eq!(&*sent, hello);
    assert!(h.facts().as_slice().contains(&Fact::Reported { watchers: 2, kept: Kept::Nothing }));
}

#[test]
fn a_phase_change_streams_to_the_watchers_of_its_item_and_of_its_board() {
    let mut h = Harness::new(LIMITS);
    h.watch(1, Subject::Item(ITEM));
    h.watch(2, Subject::Board(0));
    h.watch(3, Subject::Board(1));
    h.watch(4, Subject::Item(OTHER_ITEM));
    let sent = h.step(Event::Phase { item: ITEM, repository: 0, phase: 7 });
    let running = [deliver(1, 0, Box::new([change(ITEM, 7)])), deliver(2, 0, Box::new([change(ITEM, 7)]))];
    assert_eq!(&*sent, running);
}

#[test]
fn what_comes_meanwhile_goes_in_one_delivery_once_the_last_has_ended() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    assert_eq!(&*h.report(RUN, b"a"), [deliver(1, 0, Box::new([text(RUN, 0, b"a")]))]);
    assert!(h.report(RUN, b"b").is_empty(), "it waits for the delivery in flight");
    assert!(h.report(RUN, b"c").is_empty());
    assert_eq!(&*h.delivered(1), [deliver(1, 0, Box::new([text(RUN, 0, b"b"), text(RUN, 0, b"c")]))]);
    assert!(h.delivered(1).is_empty(), "caught up");
    let sent = h.report(RUN, b"d");
    assert_eq!(&*sent, [deliver(1, 0, Box::new([text(RUN, 0, b"d")]))], "a caught up watcher gets it at once");
    assert!(h.delivered(1).is_empty());
    assert!(h.delivered(1).is_empty(), "a delivery that ended twice is dropped");
}

#[test]
fn a_slow_watcher_is_told_what_it_missed_and_catches_up_from_the_next_chunk() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Item(ITEM));
    h.report(RUN, b"1");
    // The first watcher keeps up; the second does not.
    assert!(h.delivered(1).is_empty());
    for chunk in [b"2", b"3", b"4", b"5"] {
        let sent = h.report(RUN, chunk);
        assert_eq!(&*sent, [deliver(1, 0, Box::new([text(RUN, 0, chunk)]))]);
        assert!(h.delivered(1).is_empty());
    }
    // Its backlog held 2 and 3; 4 overflowed it, so 2 and 3 were missed.
    assert_eq!(&*h.delivered(2), [deliver(2, 2, Box::new([text(RUN, 0, b"4"), text(RUN, 0, b"5")]))]);
    assert!(h.facts().as_slice().contains(&Fact::Overflowed { missed: 2 }));
    assert!(h.delivered(2).is_empty());
    let both = [deliver(1, 0, Box::new([text(RUN, 0, b"6")])), deliver(2, 0, Box::new([text(RUN, 0, b"6")]))];
    assert_eq!(&*h.report(RUN, b"6"), both);
    assert_eq!(h.model.lost(), lost(0, 0, 2, 0));
}

#[test]
fn what_a_stream_did_not_take_is_told_as_missed_with_the_next_delivery() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.report(RUN, b"a");
    assert!(h.report(RUN, b"b").is_empty());
    // "a" was not taken: it is missed, told with "b".
    assert_eq!(&*h.failed(1), [deliver(1, 1, Box::new([text(RUN, 0, b"b")]))]);
    // Neither was that one, nor what it told: told with the next chunk.
    assert!(h.failed(1).is_empty());
    assert_eq!(&*h.report(RUN, b"c"), [deliver(1, 2, Box::new([text(RUN, 0, b"c")]))]);
    assert!(h.delivered(1).is_empty());
    assert_eq!(h.model.lost(), lost(0, 0, 2, 0));
    assert!(h.facts().as_slice().contains(&Fact::Undelivered { chunks: 1 }));
}

#[test]
fn a_report_dropped_is_told_as_missed_to_those_who_would_have_had_it() {
    let mut h = Harness::following(LIMITS, policy(Capture::Content));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Item(ITEM));
    assert!(h.report(RUN, &[b'x'; 17]).is_empty());
    assert!(h.facts().as_slice().contains(&Fact::Dropped { dropped: Dropped::Oversized }));
    let sent = h.report(RUN, b"a");
    assert_eq!(&*sent, [deliver(1, 1, Box::new([text(RUN, 0, b"a")])), deliver(2, 1, Box::new([text(RUN, 0, b"a")]))]);
    assert_eq!(h.model.lost(), lost(0, 1, 2, 0));
}

#[test]
fn a_run_turned_away_is_refused_as_unfollowed_and_its_item_told_what_it_missed() {
    let mut h = Harness::following(LIMITS, policy(Capture::Content));
    let third = Token::new(3);
    h.step(Event::Started { run: OTHER_RUN, attempt: ATTEMPT, item: ITEM, policy: policy(Capture::Nothing) });
    h.step(Event::Started { run: third, attempt: ATTEMPT, item: OTHER_ITEM, policy: policy(Capture::Nothing) });
    assert_eq!(h.facts().as_slice(), [Fact::Followed, Fact::Unfollowed]);
    let watcher = Token::new(1);
    let refused = h.step(Event::Watch { watcher, subject: Subject::Run(third), snapshot: Box::new([]) });
    assert_eq!(&*refused, [Request::Refused { watcher, refusal: Refusal::Unfollowed }]);
    h.watch(1, Subject::Item(OTHER_ITEM));
    assert!(h.report(third, b"a").is_empty(), "a run not followed reaches no one");
    let sent = h.step(Event::Phase { item: OTHER_ITEM, repository: 0, phase: 2 });
    assert_eq!(&*sent, [deliver(1, 1, Box::new([change(OTHER_ITEM, 2)]))]);
    assert_eq!((h.model.lost(), h.model.batched()), (lost(1, 1, 1, 0), 0));
    // Once it has finished, it is forgotten.
    h.step(Event::Finished { run: third });
    let unknown = h.step(Event::Watch { watcher: Token::new(2), subject: Subject::Run(third), snapshot: Box::new([]) });
    assert_eq!(&*unknown, [Request::Refused { watcher: Token::new(2), refusal: Refusal::Unknown }]);
}

#[test]
fn stopping_watching_ends_the_watch_once_its_delivery_has() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Run(RUN));
    assert_eq!(&*h.step(Event::Unwatch { watcher: Token::new(1) }), [ended(1, End::Unwatched)]);
    h.report(RUN, b"a");
    h.report(RUN, b"b");
    assert!(h.step(Event::Unwatch { watcher: Token::new(2) }).is_empty(), "its delivery is in flight");
    assert!(h.report(RUN, b"c").is_empty(), "nothing more comes to it");
    assert!(h.step(Event::Unwatch { watcher: Token::new(2) }).is_empty(), "a second stop changes nothing");
    assert_eq!(&*h.failed(2), [ended(2, End::Unwatched)], "what waited is dropped");
    assert_eq!(h.model.watchers(), 0);
    // Its token is the parent's again.
    h.watch(2, Subject::Item(ITEM));
}

#[test]
fn a_finished_run_ends_its_watchers_once_they_have_had_what_waited() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Run(RUN));
    h.watch(3, Subject::Item(ITEM));
    h.report(RUN, b"a");
    assert!(h.delivered(1).is_empty());
    assert!(h.delivered(3).is_empty());
    h.report(RUN, b"b");
    assert!(h.delivered(1).is_empty());
    assert!(h.delivered(3).is_empty());
    // The first is caught up, the second has "b" waiting.
    assert_eq!(&*h.step(Event::Finished { run: RUN }), [ended(1, End::Finished)]);
    // The run starts again, for another attempt: what it reports reaches the
    // item's watcher, and the ending watch counts it missed.
    let again = Token::new(101);
    h.step(Event::Started { run: RUN, attempt: again, item: ITEM, policy: policy(Capture::Nothing) });
    let c = Chunk::Report { run: RUN, attempt: again, kind: Kind::Text, at: Time::ZERO, content: Box::from(*b"c") };
    assert_eq!(&*h.report(RUN, b"c"), [deliver(3, 0, Box::new([c]))]);
    assert_eq!(&*h.delivered(2), [deliver(2, 1, Box::new([text(RUN, 0, b"b")]))]);
    assert_eq!(&*h.delivered(2), [ended(2, End::Finished)]);
    // A stop that crossed the end is dropped.
    assert!(h.step(Event::Unwatch { watcher: Token::new(1) }).is_empty());
    assert_eq!((h.model.watchers(), h.model.runs()), (1, 1));
}

#[test]
fn a_watch_that_both_stops_and_sees_its_run_finish_ends_once_as_unwatched() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Run(RUN));
    h.watch(2, Subject::Run(RUN));
    h.report(RUN, b"a");
    h.report(RUN, b"b");
    // The first is stopped, then its run finishes; the second the other way.
    assert!(h.step(Event::Unwatch { watcher: Token::new(1) }).is_empty());
    assert!(h.step(Event::Finished { run: RUN }).is_empty());
    assert!(h.step(Event::Unwatch { watcher: Token::new(2) }).is_empty());
    assert_eq!(&*h.delivered(1), [ended(1, End::Unwatched)]);
    assert_eq!(&*h.delivered(2), [ended(2, End::Unwatched)], "what waited is dropped");
}

#[test]
fn a_run_started_again_takes_its_attempt_item_and_policy_anew() {
    let mut h = Harness::following(LIMITS, policy(Capture::Nothing));
    h.watch(1, Subject::Item(OTHER_ITEM));
    let attempt = Token::new(101);
    let again = Event::Started { run: RUN, attempt, item: OTHER_ITEM, policy: policy(Capture::Shape) };
    assert!(h.step(again).is_empty());
    let a = Chunk::Report { run: RUN, attempt, kind: Kind::Text, at: Time::ZERO, content: Box::from(*b"a") };
    assert_eq!(&*h.report(RUN, b"a"), [deliver(1, 0, Box::new([a]))]);
    let sent = h.at(1);
    let (_, records) = appended(&sent);
    assert_eq!(records, [Record { attempt, ..record(0, 1, None) }]);
    assert_eq!(h.model.runs(), 1);
}

#[test]
fn traces_keep_what_the_policy_says_of_each_kind() {
    let kept = Policy { text: Capture::Content, progress: Capture::Shape, ..policy(Capture::Nothing) };
    let mut h = Harness::following(LIMITS, kept);
    for (kind, content) in [(Kind::Text, b"words"), (Kind::Progress, b"turn2"), (Kind::Call, b"llm00")] {
        assert!(h.step(Event::Reported { run: RUN, kind, content: Box::from(*content) }).is_empty());
    }
    let traced = [
        Fact::Reported { watchers: 0, kept: Kept::Content },
        Fact::Reported { watchers: 0, kept: Kept::Shape },
        Fact::Reported { watchers: 0, kept: Kept::Nothing },
    ];
    assert_eq!(h.facts().as_slice(), traced);
    // The batch goes once its oldest has waited the flush.
    let sent = h.at(1);
    let (_, records) = appended(&sent);
    let progress = Record { kind: Kind::Progress, ..record(0, 5, None) };
    assert_eq!(records, [record(0, 5, Some(Box::from(*b"words"))), progress]);
}

#[test]
fn a_full_batch_goes_at_once_and_one_that_cannot_take_a_report_goes_first() {
    let mut h = Harness::following(LIMITS, policy(Capture::Content));
    h.report(RUN, b"a");
    h.report(RUN, b"b");
    let sent = h.report(RUN, b"c");
    let (_, records) = appended(&sent);
    assert_eq!(records.len(), 3, "a batch of as many records as it holds goes at once");
    // 32 bytes a batch: two of 16 fill it, and a third goes in the next.
    h.report(RUN, &[b'x'; 16]);
    assert!(h.report(RUN, &[b'y'; 16]).is_empty());
    let sent = h.report(RUN, b"z");
    let (_, records) = appended(&sent);
    assert_eq!(records.len(), 2);
    assert_eq!(h.model.batched(), 1);
}

#[test]
fn reports_are_lost_while_the_store_is_behind_and_never_retried() {
    let limits = Limits { appends: 1, ..LIMITS };
    let mut h = Harness::following(limits, policy(Capture::Shape));
    for _ in 0..2_u32 {
        h.report(RUN, b"a");
    }
    let (first, _) = appended(&h.report(RUN, b"a"));
    // The next batch fills while the first is in flight, and then loses
    // what comes.
    for _ in 0..3_u32 {
        assert!(h.report(RUN, b"b").is_empty());
    }
    assert!(h.report(RUN, b"c").is_empty());
    assert!(h.facts().as_slice().contains(&Fact::Reported { watchers: 0, kept: Kept::Lost }));
    // The store fails the first: it is not sent again, and the next goes.
    let sent = h.step(Event::Appended { owner: first, done: false });
    let (second, records) = appended(&sent);
    assert_eq!(records.len(), 3);
    assert!(h.step(Event::Appended { owner: second, done: true }).is_empty());
    assert_eq!((h.model.ops(), h.model.batched()), (0, 0));
    assert_eq!(h.model.lost(), lost(0, 0, 0, 4), "one lost for room, three in the append that failed");
}

#[test]
fn a_batch_due_while_the_store_is_behind_goes_once_it_has_room() {
    let limits = Limits { appends: 1, ..LIMITS };
    let mut h = Harness::following(limits, policy(Capture::Shape));
    h.report(RUN, b"a");
    let (first, _) = appended(&h.at(1));
    h.report(RUN, b"b");
    assert!(h.at(2).is_empty(), "due, but the store has no room");
    assert_eq!(h.model.next_deadline(), Some(at(10)), "only the sweep runs");
    let sent = h.step(Event::Appended { owner: first, done: true });
    let (_, records) = appended(&sent);
    assert_eq!(records, [record(1, 1, None)]);
}

#[test]
fn operations_that_end_within_an_iteration_hold_a_batch_and_a_sweep_for_the_next_instant() {
    let limits = Limits { appends: 1, records: 1, ..LIMITS };
    let mut h = Harness::following(limits, policy(Capture::Shape));
    h.env.now = at(10);
    // Four appends answered within the iteration hold every slot until the
    // reclaim point.
    for _ in 0..4_u32 {
        h.held(Event::Reported { run: RUN, kind: Kind::Text, content: Box::from(*b"a") });
        let Some(Request::Append { owner, .. }) = h.out.pop() else { panic!("a batch of one goes at once") };
        h.held(Event::Appended { owner, done: true });
        h.held(Event::Appended { owner, done: true });
    }
    h.held(Event::Reported { run: RUN, kind: Kind::Text, content: Box::from(*b"b") });
    assert_eq!(h.model.batched(), 1, "no slot is free");
    // The sweep is due too, and waits as well.
    assert!(h.model.is_due(h.env.now));
    fire(&mut h.model, &h.env, &mut h.out);
    assert!(h.drain().is_empty());
    let next = at(10).saturating_add(Duration::from_nanos(1));
    assert_eq!(h.model.next_deadline(), Some(next));
    let fired = h.at_time(next);
    let [Request::Append { .. }, Request::Expire { .. }] = &*fired else { panic!("both go: {fired:?}") };
}

#[test]
fn a_new_model_sweeps_what_an_earlier_engine_kept() {
    let mut h = Harness::new(LIMITS);
    let started = at(100);
    h.model = Model::new(&LIMITS, started);
    assert!(h.model.is_sweeping());
    assert_eq!(h.model.next_deadline(), Some(at(110)), "a sweep a period after it starts");
    // It sweeps until an expire covers what was reported before it started.
    let mut swept = 0;
    for secs in (110..=200).step_by(10) {
        if !h.model.is_sweeping() {
            break;
        }
        let (owner, before) = expire(&h.at(secs));
        assert_eq!(before, at(secs.checked_sub(60).unwrap()));
        assert!(h.step(Event::Expired { owner, done: true }).is_empty());
        swept = secs;
    }
    assert_eq!(swept, 170, "the first expire past the minute after it started");
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn the_store_is_swept_while_it_may_hold_records_and_no_longer() {
    let mut h = Harness::following(LIMITS, policy(Capture::Shape));
    h.env.now = at(5);
    h.report(RUN, b"a");
    let (append, _) = appended(&h.at(6));
    assert!(h.step(Event::Appended { owner: append, done: true }).is_empty());
    assert!(h.model.is_sweeping());
    // A sweep every ten seconds, of what is past the minute's retention.
    let (owner, before) = expire(&h.at(10));
    assert_eq!(before, Time::ZERO);
    assert_eq!(h.model.next_deadline(), None, "no sweep while one is in flight");
    assert!(h.step(Event::Expired { owner, done: true }).is_empty());
    assert!(h.step(Event::Expired { owner, done: true }).is_empty(), "a second end is dropped");
    assert_eq!(h.model.next_deadline(), Some(at(20)), "the record is not covered yet");
    for secs in [20, 30, 40, 50, 60] {
        let (owner, _) = expire(&h.at(secs));
        assert!(h.step(Event::Expired { owner, done: true }).is_empty());
    }
    // The sweep at 70 covers what was reported at 5; but it fails, and the
    // next one does not.
    let (owner, before) = expire(&h.at(70));
    assert_eq!(before, at(10));
    assert!(h.step(Event::Expired { owner, done: false }).is_empty());
    let (owner, _) = expire(&h.at(80));
    assert!(h.step(Event::Expired { owner, done: true }).is_empty());
    assert!(!h.model.is_sweeping());
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn a_batch_sent_while_an_expire_is_in_flight_keeps_the_sweep_going() {
    let limits = Limits { retention: Duration::from_secs(1), ..LIMITS };
    let mut h = Harness::following(limits, policy(Capture::Shape));
    h.report(RUN, b"a");
    let (append, _) = appended(&h.at(1));
    assert!(h.step(Event::Appended { owner: append, done: true }).is_empty());
    let (owner, before) = expire(&h.at(10));
    assert_eq!(before, at(9));
    // An old report, sent after the expire was asked for.
    h.env.now = at(5);
    h.report(RUN, b"b");
    let (append, _) = appended(&h.at(11));
    assert!(h.step(Event::Appended { owner: append, done: true }).is_empty());
    assert!(h.step(Event::Expired { owner, done: true }).is_empty());
    assert!(h.model.is_sweeping(), "the store may have taken it after the expire");
    let (owner, _) = expire(&h.at(20));
    assert!(h.step(Event::Expired { owner, done: true }).is_empty());
    assert!(!h.model.is_sweeping());
}

#[test]
fn a_terminal_that_names_no_operation_in_flight_is_dropped() {
    let mut h = Harness::following(LIMITS, policy(Capture::Shape));
    h.report(RUN, b"a");
    let (append, _) = appended(&h.at(1));
    // A duplicate, within the iteration and after it.
    h.held(Event::Appended { owner: append, done: true });
    h.held(Event::Appended { owner: append, done: false });
    assert!(h.drain().is_empty());
    assert!(h.step(Event::Appended { owner: append, done: true }).is_empty(), "ended, and reclaimed");
    assert!(h.delivered(9).is_empty(), "a delivery to no watch");
    assert!(h.step(Event::Unwatch { watcher: Token::new(9) }).is_empty());
    assert_eq!((h.model.ops(), h.model.lost()), (0, lost(0, 0, 0, 0)));
}

/// What a watcher of a run that reports four times sees, with room for
/// `facts` facts.
fn watched(facts: u32) -> Box<[Request]> {
    let mut h = Harness::new(Limits { facts, ..LIMITS });
    h.step(Event::Started { run: RUN, attempt: ATTEMPT, item: ITEM, policy: policy(Capture::Content) });
    h.watch(1, Subject::Run(RUN));
    let mut seen = List::with_capacity(16);
    for content in [b"a", b"b", b"c", b"d"] {
        for request in h.report(RUN, content) {
            seen.push(request).unwrap();
        }
    }
    for request in h.delivered(1) {
        seen.push(request).unwrap();
    }
    seen.into_boxed()
}

#[test]
fn facts_change_nothing() {
    assert_eq!(watched(0), watched(64));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    let backlogs = u64::from(LIMITS.watchers * LIMITS.backlog * LIMITS.report_bytes);
    assert!(bound > backlogs + u64::from(LIMITS.batch_bytes), "it counts every backlog full, and the batch");
    let more = worst_case(&Limits { watchers: 8, ..LIMITS }).expect("fits");
    assert!(more > bound, "a watcher more is more");
    let longer = worst_case(&Limits { backlog: 64, ..LIMITS }).expect("fits");
    assert!(longer > bound, "a longer backlog is more");
    let bigger = worst_case(&Limits { report_bytes: 32, ..LIMITS }).expect("fits");
    assert!(bigger > bound, "a bigger report is more");
    let snapshot = worst_case(&Limits { snapshot_bytes: 4096, ..LIMITS }).expect("fits");
    assert!(snapshot > bound, "a bigger snapshot is more");
    assert_eq!(worst_case(&Limits { watchers: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { backlog: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { records: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { appends: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { sweep: Duration::ZERO, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { report_bytes: 33, ..LIMITS }), None, "a batch holds a report at least");
    assert_eq!(worst_case(&Limits { watchers: u32::MAX, backlog: u32::MAX, ..LIMITS }), None);
}
