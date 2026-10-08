use skein_lib::{Queue, Token};
use temper_engine_domain::{
    self as root, Counters, Decision, Delivery, Family, Journal, JournalOutput as Output, Record, Write,
};
use temper_engine_domain_world::commits::{HEADER, LIMITS, Referee, Store, World, random};

#[test]
fn a_cumulative_completion_releases_decisions_in_order_one_at_a_time() {
    let mut world = World::new();
    world.decision(1, b"first", true, true);
    world.decision(2, b"second", true, false);
    world.decision(3, b"read", false, false);
    world.apply(false);
    world.apply(true);
    for _ in 0_u32..3 {
        world.ready();
    }
    assert!(world.referee.done());
    assert_eq!(world.referee.judged, 5);
}

#[test]
fn held_outputs_wait_behind_each_unanswered_commit() {
    let mut world = World::new();
    world.decision(1, b"first", true, false);
    world.decision(2, b"second", true, false);

    world.ready();
    assert_eq!(world.referee.judged, 2, "only the two commits reached the store");
    world.apply(true);
    world.ready();
    assert_eq!(world.referee.judged, 3, "only the first held output was released");
    world.ready();
    assert_eq!(world.referee.judged, 3, "the second commit is still unanswered");

    world.apply(true);
    world.ready();
    assert_eq!(world.referee.judged, 4);
    assert!(world.referee.done());
}

#[test]
fn a_route_past_its_reserved_room_stops_with_earlier_outputs_held() {
    let mut journal = Journal::from_durable(&root::journal_limits(&LIMITS), HEADER.commits);
    let mut counters = Counters::new(HEADER);
    let mut out = Queue::with_capacity(1);
    let mut first = Decision::reserve(&mut journal, &LIMITS).expect("first route admitted");
    first
        .deliver(&LIMITS, Delivery::AcknowledgeTurn { channel: Token::new(7), task: 1, attempt: 1, turn: 1 })
        .expect("held output fits");
    root::accept(&mut journal, &mut counters, &LIMITS, first, &mut out).expect("first route accepted");
    assert!(out.is_empty());

    let mut overrun = Decision::reserve(&mut journal, &LIMITS).expect("second route admitted");
    for turn in 1..LIMITS.writes {
        overrun
            .write(
                &LIMITS,
                Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(root::TurnRecord {
                    task: 1,
                    attempt: 1,
                    turn,
                    spent: u64::from(turn),
                    read: None,
                    at: skein_lib::Wall::from_nanos(u64::from(turn)),
                    transcript: Box::new([]),
                })))),
            )
            .expect("reserved write room");
    }
    assert!(
        overrun
            .write(
                &LIMITS,
                Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(root::TurnRecord {
                    task: 1,
                    attempt: 1,
                    turn: LIMITS.writes,
                    spent: u64::from(LIMITS.writes),
                    read: None,
                    at: skein_lib::Wall::from_nanos(u64::from(LIMITS.writes)),
                    transcript: Box::new([]),
                })))),
            )
            .is_err()
    );
    root::accept(&mut journal, &mut counters, &LIMITS, overrun, &mut out).expect("overrun reports stop");
    assert!(journal.stopped());
    root::resume(&mut journal, &mut out);
    assert!(out.is_empty(), "no held output leaves a stopped journal");
}

#[test]
fn a_lost_completion_recovers_the_whole_header_and_transcript_without_reapplying() {
    let mut world = World::new();
    world.decision(1, b"durable transcript", true, true);
    world.apply(false);
    let mut before = Queue::with_capacity(1);
    root::resume(&mut world.journal, &mut before);
    assert!(before.is_empty());
    let rows = world.store.rows.clone();
    let header = world.store.header();
    let mut recovered = Journal::from_durable(&root::journal_limits(&LIMITS), header.commits);
    let mut counters = Counters::new(header);
    let mut out = Queue::with_capacity(1);
    let mut replay = Decision::new(&LIMITS);
    replay
        .deliver(&LIMITS, Delivery::AcknowledgeTurn { channel: Token::new(7), task: 1, attempt: 1, turn: 1 })
        .expect("bounded replay acknowledgement");
    root::accept(&mut recovered, &mut counters, &LIMITS, replay, &mut out).expect("recovered decision room");
    assert!(out.is_empty());
    root::resume(&mut recovered, &mut out);
    assert_eq!(
        out.pop(),
        Some(Output::Deliver(Delivery::AcknowledgeTurn { channel: Token::new(7), task: 1, attempt: 1, turn: 1 }))
    );
    assert_eq!(world.store.rows, rows);
    assert_eq!(root::fresh(&mut counters, Family::Task), Some(2));
}

#[test]
fn random_store_lag_replays_the_identical_trace() {
    for seed in [3_u64, 17, 29] {
        let first = random(seed);
        let replay = random(seed);
        assert_eq!(first.trace, replay.trace, "seed {seed}");
        assert_eq!(first.store.rows, replay.store.rows);
    }
}

#[test]
fn the_referee_rejects_early_duplicate_or_reordered_acknowledgements() {
    let ack = |turn| Output::Deliver(Delivery::AcknowledgeTurn { channel: Token::new(7), task: 1, attempt: 1, turn });
    let mut judge = Referee::new();
    let mut store = Store::new();
    judge.decision(1, b"first", true, false);
    assert_eq!(judge.observe(&mut store, ack(1)), Err("before durability"));
    store.applied = 1;
    assert_eq!(judge.observe(&mut store, ack(2)), Err("delivery order or fence"));
    assert_eq!(judge.observe(&mut store, ack(1)), Ok(()));
    assert_eq!(judge.observe(&mut store, ack(1)), Err("duplicate delivery"));
    assert_eq!(
        judge.observe(&mut store, Output::Deliver(Delivery::Cancel { channel: Token::new(7), task: 1, attempt: 1 })),
        Err("delivery shape")
    );
}

#[test]
fn a_failed_fake_store_transaction_changes_no_rows_and_stops_waiting_releases() {
    let mut world = World::new();
    world.decision(1, b"kept", true, true);
    world.apply(true);
    world.ready();
    let rows = world.store.rows.clone();
    world.decision(2, b"must not be partly applied", true, true);
    world.decision(3, b"later", true, true);
    let failed = world.store.pending.pop_front().expect("failing transaction");
    assert_eq!(failed.0, 2);
    drop(failed);
    world.store.pending.clear();
    let mut out = Queue::with_capacity(1);
    root::uncommitted(&mut world.journal, 2, &mut out);
    assert_eq!(out.pop(), Some(Output::Stop));
    root::committed(&mut world.journal, 3);
    root::resume(&mut world.journal, &mut out);
    assert!(out.is_empty());
    assert_eq!(world.store.rows, rows);
    let recovered = Counters::new(world.store.header());
    assert_eq!(recovered.deployment().commits, 1);
    assert_eq!(recovered.deployment().tasks, 1);
}

#[test]
fn the_referee_rejects_missing_records_wrong_numbers_and_unsolicited_commits() {
    for wrong_number in [false, true] {
        let mut judge = Referee::new();
        let mut store = Store::new();
        judge.decision(1, b"first", true, true);
        let writes = Box::new([Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Deployment(
            root::Deployment { tasks: 1, commits: 1, ..HEADER },
        ))))]);
        let expected = if wrong_number { "commit number" } else { "atomic records" };
        assert_eq!(
            judge.observe(&mut store, Output::Commit { number: if wrong_number { 2 } else { 1 }, writes }),
            Err(expected)
        );
    }
    let mut judge = Referee::new();
    let mut store = Store::new();
    assert_eq!(
        judge.observe(&mut store, Output::Commit { number: 1, writes: Box::new([]) }),
        Err("unsolicited commit")
    );
    assert_eq!(judge.observe(&mut store, Output::Stop), Err("unexpected stop"));
}
