use skein_lib::{Queue, Token, Wall};
use temper_engine_domain::{
    self as root, Counters, Decision, Delivery, Journal, JournalOutput as Output, Record, TurnRecord, Write,
};
use temper_engine_domain_world::commits::{HEADER, LIMITS};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

#[test]
fn retired_and_abandoned_load_slots_and_a_partial_page_fit_without_payload_clones() {
    use root::{Range, loads};
    let slot = u32::try_from(core::mem::size_of::<Record>()).expect("small slot");
    let limits =
        loads::Limits { loads: 2, rows: 4, bytes: slot * 2 + 8, reply_bytes: 4 * (slot + 4), transcript_bytes: 4 };
    let mut out = Queue::<loads::Request>::with_capacity(1);
    let meter = Meter::new();
    let mut domain = loads::Loads::new(&limits);
    let incoming: Box<[Record]> = (1..=4)
        .map(|turn| {
            Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(TurnRecord {
                task: 1,
                attempt: 1,
                turn,
                spent: 0,
                read: None,
                at: Wall::EPOCH,
                transcript: vec![b'x'; 4].into_boxed_slice(),
            })))
        })
        .collect();
    let mut addresses = [core::ptr::null(); 4];
    for (address, row) in addresses.iter_mut().zip(incoming.iter()) {
        let Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(row))) = row else {
            panic!("turn");
        };
        *address = row.transcript.as_ptr();
    }
    let first = loads::begin(
        &mut domain,
        Token::new(1),
        Range::Core(temper_engine_domain::CoreRange::Turns { task: 1, attempt: 1 }),
        None,
        4,
        &mut out,
    )
    .expect("first slot");
    drop(out.pop().expect("first IO"));
    let second = loads::begin(
        &mut domain,
        Token::new(2),
        Range::Core(temper_engine_domain::CoreRange::Deployment),
        None,
        1,
        &mut out,
    )
    .expect("second slot");
    drop(out.pop().expect("second IO"));
    loads::abandon(&mut domain, second);
    meter.start();
    loads::loaded(&mut domain, first, incoming, None, &mut out);
    let measured = meter.end();
    let loads::Request::Loaded { rows, cut, .. } = out.pop().expect("partial page") else {
        panic!("loaded");
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(cut.expect("cut").rows, 2);
    for (row, original) in rows.iter().zip(addresses) {
        let Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(row))) = row else {
            panic!("turn");
        };
        assert_eq!(row.transcript.as_ptr(), original, "owned bytes moved directly");
    }
    drop(rows);
    meter.check(measured, loads::worst_case(&limits).expect("valid bound"), limits);
    assert!(
        loads::begin(
            &mut domain,
            Token::new(3),
            Range::Core(temper_engine_domain::CoreRange::Deployment),
            None,
            1,
            &mut out
        )
        .is_none(),
        "retired and abandoned both retain slots"
    );
    loads::reclaim(&mut domain);
    assert!(
        loads::begin(
            &mut domain,
            Token::new(3),
            Range::Core(temper_engine_domain::CoreRange::Deployment),
            None,
            1,
            &mut out
        )
        .is_some()
    );
    drop(out.pop().expect("reclaimed first slot"));
    loads::unloaded(&mut domain, second, &mut out);
    assert!(out.is_empty(), "abandoned terminal does not wake its old waiter");
}

#[test]
fn partial_delivery_transfer_never_allocates_a_second_shrinking_container() {
    let limits = root::JournalLimits {
        commits: 1,
        held: 1000,
        writes: 1,
        deliveries: 1000,
        transcript_bytes: 0,
        result_bytes: 0,
        run_bytes: 1,
    };
    let mut out = Queue::<Output>::with_capacity(1);
    let meter = Meter::new();
    let mut journal = Journal::from_durable(&root::journal_limits(&limits), HEADER.commits);
    let mut counters = Counters::new(HEADER);
    let mut decision = Decision::new(&limits);
    for task in 1_u64..1000 {
        decision
            .deliver(&limits, Delivery::Acknowledge { channel: Token::new(7), task, attempt: 1 })
            .expect("partial delivery bound");
    }
    meter.start();
    root::accept(&mut journal, &mut counters, &limits, decision, &mut out).expect("reserved whole decision");
    let measured = meter.end();
    assert!(out.is_empty(), "clean header and no writes");
    meter.check(measured, root::worst_case(&limits).expect("valid bounds"), limits);
    assert!(!journal.idle());
}

#[test]
fn partial_write_transfer_moves_values_without_shrinking_the_source() {
    let limits = root::JournalLimits {
        commits: 1,
        held: 1,
        writes: 128,
        deliveries: 1,
        transcript_bytes: 0,
        result_bytes: 0,
        run_bytes: 1,
    };
    let mut out = Queue::<Output>::with_capacity(1);
    let meter = Meter::new();
    let mut journal = Journal::from_durable(&root::journal_limits(&limits), HEADER.commits);
    let mut counters = Counters::new(HEADER);
    let mut decision = Decision::new(&limits);
    for turn in 1_u32..127 {
        decision
            .write(
                &limits,
                Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(TurnRecord {
                    task: 1,
                    attempt: 1,
                    turn,
                    spent: 0,
                    read: None,
                    at: Wall::EPOCH,
                    transcript: Box::new([]),
                })))),
            )
            .expect("partial write bound");
    }
    meter.start();
    root::accept(&mut journal, &mut counters, &limits, decision, &mut out).expect("reserved whole decision");
    let measured = meter.end();
    let Output::Commit { writes, .. } = out.pop().expect("one commit") else {
        panic!("commit");
    };
    assert_eq!(writes.len(), 127);
    drop(writes);
    meter.check(measured, root::worst_case(&limits).expect("valid bounds"), limits);
}

#[test]
fn full_decisions_and_maximum_held_results_fit_the_declared_root_journal_bound() {
    let mut out = Queue::<Output>::with_capacity(1);
    let meter = Meter::new();
    let mut journal = Journal::from_durable(&root::journal_limits(&LIMITS), HEADER.commits);
    let mut counters = Counters::new(HEADER);
    let bound = root::worst_case(&LIMITS).expect("valid journal limits");
    for commit in 1_u64..=u64::from(LIMITS.commits) {
        meter.start();
        let mut decision = Decision::new(&LIMITS);
        for turn in 1..LIMITS.writes {
            decision
                .write(
                    &LIMITS,
                    Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(TurnRecord {
                        task: commit,
                        attempt: 1,
                        turn,
                        spent: 0,
                        read: None,
                        at: Wall::EPOCH,
                        transcript: vec![b'x'; usize::try_from(LIMITS.transcript_bytes).expect("small transcript")]
                            .into_boxed_slice(),
                    })))),
                )
                .expect("maximum admitted turn");
        }
        // Replace a fully allocated value while preserving the decision's
        // key position; the incoming value and old value are both live first.
        decision
            .write(
                &LIMITS,
                Write::Save(Record::Core(jig_core::Record::Core(jig_core::CoreRecord::Turn(TurnRecord {
                    task: commit,
                    attempt: 1,
                    turn: 1,
                    spent: 1,
                    read: None,
                    at: Wall::EPOCH,
                    transcript: vec![b'y'; usize::try_from(LIMITS.transcript_bytes).expect("small transcript")]
                        .into_boxed_slice(),
                })))),
            )
            .expect("bounded replacement");
        for task in 1_u64..=u64::from(LIMITS.deliveries) {
            decision
                .deliver(
                    &LIMITS,
                    Delivery::Result {
                        person: 1,
                        task,
                        words: vec![b'x'; usize::try_from(LIMITS.result_bytes).expect("small result")]
                            .into_boxed_slice(),
                    },
                )
                .expect("maximum admitted result");
        }
        root::accept(&mut journal, &mut counters, &LIMITS, decision, &mut out).expect("whole decision reserved");
        let measured = meter.end();
        drop(out.pop().expect("one commit"));
        meter.check(measured, bound, LIMITS);
    }
    assert!(!root::takes(&journal, &LIMITS));
    root::committed(&mut journal, u64::from(LIMITS.commits));
    for _ in 0..LIMITS.held {
        meter.start();
        root::resume(&mut journal, &mut out);
        let measured = meter.end();
        drop(out.pop().expect("one bounded ready result"));
        meter.check(measured, bound, LIMITS);
    }
    assert!(journal.idle());
    assert!(root::takes(&journal, &LIMITS));
}

#[test]
fn real_root_state_and_complete_walking_handoffs_fit_the_declared_counted_bound() {
    use temper_engine_domain_world::walking::{Settings, World, limits};
    let limits = limits();
    let bound = root::engine::worst_case(&limits).expect("all child and root bounds checked");
    for settings in [Settings::calm(73), Settings::terminal(74)] {
        let meter = Meter::new();
        meter.start();
        let mut world = World::new(settings);
        world.run();
        let measured = meter.end();
        // The meter also sees the independent fake's durable map and script
        // observations. Their tiny footprint only makes this stricter: root
        // scratch queues and every real restored child are exercised together.
        meter.check(measured, bound, limits);
    }
}

#[test]
fn held_reasons_history_queries_and_real_escalation_handoffs_fit_counted_root_memory() {
    use temper_engine_domain_world::{escalation, escalation_referee::Story};
    let limits = escalation::limits();
    let bound = root::engine::worst_case(&limits).expect("all escalation retained and transient ownership priced");
    for story in [Story::Release, Story::PassRelease, Story::RaceReject] {
        let meter = Meter::new();
        meter.start();
        let mut world = escalation::World::new(escalation::Settings {
            cut: escalation::Cut::Decision,
            commit_delay: 1,
            page_delay: 1,
            ..escalation::Settings::calm(9201, story)
        });
        world.run();
        let measured = meter.end();
        // Includes the fake's durable history and independent observations as
        // well as actual root query/context/journal and restored child copies.
        meter.check(measured, bound, limits);
    }
}

#[test]
fn candidate_rosters_waiting_snapshots_keyed_history_and_recovery_fit_counted_memory() {
    use temper_engine_domain_world::roles::{Base, Settings, limits, reroute_replayed};
    let limits = limits();
    let bound = root::engine::worst_case(&limits).expect("role snapshots and every roster copy priced");
    let meter = Meter::new();
    meter.start();
    let world = reroute_replayed(
        Settings { cut: true, commit_delay: 1, page_delay: 1, ..Settings::calm(9311, Base::Requester) },
        1,
    );
    assert_eq!(world.referee.replacements, 1);
    let measured = meter.end();
    // Includes the fake store, immutable before-observers and exact replay
    // transcript in addition to real children and every root transient copy.
    meter.check(measured, bound, limits);
}
