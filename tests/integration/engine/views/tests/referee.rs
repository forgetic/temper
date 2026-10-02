//! The referee of the views' world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use temper_engine_model_views::{Capture, Chunk, End, Kind, Policy, Record, Refusal, Subject};
use temper_engine_model_views_tests::LIMITS;
use temper_engine_model_views_tests::referee::{Bounds, Seen, Views};
use temper_lib::{Duration, Time, Token};
use temper_world::{Referee, Verdict};

const RUN: u64 = 1;
const ITEM: u64 = 10;
const BOUNDS: Bounds =
    Bounds { reach: Duration::from_secs(1), end: Duration::from_secs(2), forget: Duration::from_secs(5) };

fn at(millis: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_millis(millis))
}

fn why(referee: &Referee<Views>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else {
        panic!("the referee failed the run: {:?}", referee.verdict())
    };
    failure.why
}

fn see(referee: &mut Referee<Views>, millis: u64, seen: Seen) {
    referee.observe(at(millis), seen, &mut Vec::new());
}

fn text(millis: u64, content: &[u8]) -> Chunk {
    Chunk::Report { run: Token::new(RUN), kind: Kind::Text, at: at(millis), content: content.into() }
}

/// A referee that has seen `RUN` start for `ITEM`, keeping the shape of
/// what it reports, and watcher 5 watch it.
fn watching() -> Referee<Views> {
    let mut referee = Referee::new(Views::new(LIMITS, BOUNDS));
    let shape = Policy {
        text: Capture::Shape,
        progress: Capture::Shape,
        calls: Capture::Shape,
        tools: Capture::Shape,
        usage: Capture::Shape,
    };
    see(&mut referee, 0, Seen::Started { run: RUN, item: ITEM, policy: shape });
    see(&mut referee, 0, Seen::Watch { watcher: 5, subject: Subject::Run(Token::new(RUN)) });
    see(&mut referee, 0, Seen::Watching { watcher: 5 });
    referee
}

fn report(referee: &mut Referee<Views>, millis: u64, content: &[u8]) {
    see(referee, millis, Seen::Reported { run: RUN, kind: Kind::Text, content: content.to_vec() });
}

#[test]
fn a_run_watched_and_traced_as_it_should_be_passes() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    see(&mut referee, 10, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(10, b"a")] });
    for (millis, content) in [(20, b"b"), (30, b"c"), (40, b"d")] {
        report(&mut referee, millis, content);
    }
    see(&mut referee, 50, Seen::Delivered { watcher: 5 });
    see(&mut referee, 50, Seen::Deliver { watcher: 5, missed: 2, chunks: vec![text(40, b"d")] });
    see(&mut referee, 60, Seen::Delivered { watcher: 5 });
    let records = vec![
        Record { run: Token::new(RUN), kind: Kind::Text, at: at(10), size: 1, content: None },
        Record { run: Token::new(RUN), kind: Kind::Text, at: at(30), size: 1, content: None },
    ];
    see(&mut referee, 1000, Seen::Kept { append: 1, records });
    see(&mut referee, 1100, Seen::Finished { run: RUN });
    see(&mut referee, 1100, Seen::Ended { watcher: 5, end: End::Finished });
    see(&mut referee, 31_000, Seen::Expire { before: at(1000) });
    see(&mut referee, 31_100, Seen::Forgot { before: at(1000), done: true });
    assert_eq!(referee.verdict(), Verdict::Passed);
    let views = referee.expectations();
    assert_eq!((views.chunks, views.deliveries, views.missed, views.records), (2, 2, 2, 2));
}

#[test]
fn a_chunk_delivered_twice_or_out_of_order_fails_the_run() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    see(&mut referee, 10, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(10, b"a")] });
    report(&mut referee, 20, b"b");
    report(&mut referee, 30, b"c");
    see(&mut referee, 40, Seen::Delivered { watcher: 5 });
    see(&mut referee, 40, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(30, b"c"), text(20, b"b")] });
    assert!(why(&referee).contains("delivered in order, once"), "{}", why(&referee));
}

#[test]
fn chunks_missed_and_not_told_fail_the_run() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    see(&mut referee, 10, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(10, b"a")] });
    report(&mut referee, 20, b"b");
    report(&mut referee, 30, b"c");
    see(&mut referee, 40, Seen::Delivered { watcher: 5 });
    see(&mut referee, 40, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(30, b"c")] });
    assert!(why(&referee).contains("takes all that waits"), "{}", why(&referee));
}

#[test]
fn a_chunk_that_does_not_reach_a_caught_up_watcher_in_time_fails_the_run() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    referee.fire(at(1011), &mut Vec::new());
    assert!(why(&referee).contains("Reach { watcher: 5, chunk: 1 }"), "{}", why(&referee));
}

#[test]
fn a_watch_refused_with_room_for_it_fails_the_run() {
    let mut referee = watching();
    see(&mut referee, 10, Seen::Watch { watcher: 6, subject: Subject::Board(0) });
    see(&mut referee, 10, Seen::Refused { watcher: 6, refusal: Refusal::Busy });
    assert!(why(&referee).contains("answered Some(Busy), not None"), "{}", why(&referee));
}

#[test]
fn a_watch_that_ends_with_its_delivery_in_flight_fails_the_run() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    see(&mut referee, 10, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(10, b"a")] });
    see(&mut referee, 20, Seen::Unwatch { watcher: 5 });
    see(&mut referee, 20, Seen::Ended { watcher: 5, end: End::Unwatched });
    assert!(why(&referee).contains("ends once its delivery has"), "{}", why(&referee));
}

#[test]
fn a_record_kept_past_what_the_policy_says_fails_the_run() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    let whole =
        Record { run: Token::new(RUN), kind: Kind::Text, at: at(10), size: 1, content: Some(b"a".as_slice().into()) };
    see(&mut referee, 1000, Seen::Kept { append: 1, records: vec![whole] });
    assert!(why(&referee).contains("keeps only what the policies keep"), "{}", why(&referee));
}

#[test]
fn an_expire_within_the_retention_fails_the_run() {
    let mut referee = watching();
    see(&mut referee, 40_000, Seen::Expire { before: at(20_000) });
    assert!(why(&referee).contains("forgets nothing within the retention"), "{}", why(&referee));
}

#[test]
fn a_record_kept_past_its_retention_and_a_sweep_fails_the_run() {
    let mut referee = watching();
    report(&mut referee, 10, b"a");
    see(&mut referee, 10, Seen::Deliver { watcher: 5, missed: 0, chunks: vec![text(10, b"a")] });
    see(&mut referee, 20, Seen::Delivered { watcher: 5 });
    let shape = Record { run: Token::new(RUN), kind: Kind::Text, at: at(10), size: 1, content: None };
    see(&mut referee, 1000, Seen::Kept { append: 1, records: vec![shape] });
    // Forgetting failed puts the deadline off by a sweep, no more.
    see(&mut referee, 31_000, Seen::Forgot { before: at(1000), done: false });
    referee.fire(at(36_001), &mut Vec::new());
    assert!(why(&referee).contains("Gone(1) was not met"), "{}", why(&referee));
}
