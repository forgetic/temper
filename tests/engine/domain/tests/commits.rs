use skein_lib::Queue;
use temper_engine_domain::{self as root, Decision, Delivery, Family, Journal, Output, Record, Write};
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
fn a_lost_completion_recovers_the_whole_header_and_transcript_without_reapplying() {
    let mut world = World::new();
    world.decision(1, b"durable transcript", true, true);
    world.apply(false);
    assert!(!world.journal.ready());
    let rows = world.store.rows.clone();
    let mut recovered = Journal::new(world.store.header(), &LIMITS);
    assert_eq!(recovered.durable(), 1);
    let mut out = Queue::with_capacity(1);
    let mut replay = Decision::new(&LIMITS);
    replay
        .deliver(&LIMITS, Delivery::AcknowledgeTurn { channel: 7, task: 1, attempt: 1, turn: 1 })
        .expect("bounded replay acknowledgement");
    root::accept(&mut recovered, &LIMITS, replay, &mut out).expect("recovered decision room");
    assert!(out.is_empty());
    root::resume(&mut recovered, &mut out);
    assert_eq!(
        out.pop(),
        Some(Output::Deliver(Delivery::AcknowledgeTurn { channel: 7, task: 1, attempt: 1, turn: 1 }))
    );
    assert_eq!(world.store.rows, rows);
    assert_eq!(root::fresh(&mut recovered, Family::Task), Some(2));
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
    let ack = |turn| Output::Deliver(Delivery::AcknowledgeTurn { channel: 7, task: 1, attempt: 1, turn });
    let mut judge = Referee::new();
    let mut store = Store::new();
    judge.decision(1, b"first", true, false);
    assert_eq!(judge.observe(&mut store, ack(1)), Err("before durability"));
    store.applied = 1;
    assert_eq!(judge.observe(&mut store, ack(2)), Err("delivery order or fence"));
    assert_eq!(judge.observe(&mut store, ack(1)), Ok(()));
    assert_eq!(judge.observe(&mut store, ack(1)), Err("duplicate delivery"));
    assert_eq!(
        judge.observe(&mut store, Output::Deliver(Delivery::Cancel { channel: 7, task: 1, attempt: 1 })),
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
    let recovered = Journal::new(world.store.header(), &LIMITS);
    assert_eq!(recovered.deployment().commits, 1);
    assert_eq!(recovered.deployment().tasks, 1);
}
#[test]
fn the_referee_rejects_missing_records_wrong_numbers_and_unsolicited_commits() {
    for wrong_number in [false, true] {
        let mut judge = Referee::new();
        let mut store = Store::new();
        judge.decision(1, b"first", true, true);
        let writes = Box::new([Write::Save(Record::Deployment(root::Deployment { tasks: 1, commits: 1, ..HEADER }))]);
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
