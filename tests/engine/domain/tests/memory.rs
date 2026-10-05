use skein_lib::{Queue, Token, Wall};
use temper_engine_domain::{self as root, Decision, Delivery, Journal, Output, Record, TurnRecord, Write};
use temper_engine_domain_world::commits::{HEADER, LIMITS};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

#[test]
fn partial_delivery_transfer_never_allocates_a_second_shrinking_container() {
    let limits = root::JournalLimits {
        commits: 1,
        held: 1000,
        writes: 1,
        deliveries: 1000,
        transcript_bytes: 0,
        result_bytes: 0,
    };
    let mut out = Queue::<Output>::with_capacity(1);
    let meter = Meter::new();
    let mut journal = Journal::new(HEADER, &limits);
    let mut decision = Decision::new(&limits);
    for task in 1_u64..1000 {
        decision
            .deliver(&limits, Delivery::Acknowledge { channel: Token::new(7), task, attempt: 1 })
            .expect("partial delivery bound");
    }
    meter.start();
    root::accept(&mut journal, &limits, decision, &mut out).expect("reserved whole decision");
    let measured = meter.end();
    assert!(out.is_empty(), "clean header and no writes");
    meter.check(measured, root::worst_case(&limits).expect("valid bounds"), limits);
    assert!(journal.ready());
}
#[test]
fn partial_write_transfer_moves_values_without_shrinking_the_source() {
    let limits =
        root::JournalLimits { commits: 1, held: 1, writes: 128, deliveries: 1, transcript_bytes: 0, result_bytes: 0 };
    let mut out = Queue::<Output>::with_capacity(1);
    let meter = Meter::new();
    let mut journal = Journal::new(HEADER, &limits);
    let mut decision = Decision::new(&limits);
    for turn in 1_u32..127 {
        decision
            .write(
                &limits,
                Write::Save(Record::Turn(TurnRecord {
                    task: 1,
                    attempt: 1,
                    turn,
                    spent: 0,
                    read: None,
                    at: Wall::EPOCH,
                    transcript: Box::new([]),
                })),
            )
            .expect("partial write bound");
    }
    meter.start();
    root::accept(&mut journal, &limits, decision, &mut out).expect("reserved whole decision");
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
    let mut journal = Journal::new(HEADER, &LIMITS);
    let bound = root::worst_case(&LIMITS).expect("valid journal limits");
    for commit in 1_u64..=u64::from(LIMITS.commits) {
        meter.start();
        let mut decision = Decision::new(&LIMITS);
        for turn in 1..LIMITS.writes {
            decision
                .write(
                    &LIMITS,
                    Write::Save(Record::Turn(TurnRecord {
                        task: commit,
                        attempt: 1,
                        turn,
                        spent: 0,
                        read: None,
                        at: Wall::EPOCH,
                        transcript: vec![b'x'; usize::try_from(LIMITS.transcript_bytes).expect("small transcript")]
                            .into_boxed_slice(),
                    })),
                )
                .expect("maximum admitted turn");
        }
        // Replace a fully allocated value while preserving the decision's
        // key position; the incoming value and old value are both live first.
        decision
            .write(
                &LIMITS,
                Write::Save(Record::Turn(TurnRecord {
                    task: commit,
                    attempt: 1,
                    turn: 1,
                    spent: 1,
                    read: None,
                    at: Wall::EPOCH,
                    transcript: vec![b'y'; usize::try_from(LIMITS.transcript_bytes).expect("small transcript")]
                        .into_boxed_slice(),
                })),
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
        root::accept(&mut journal, &LIMITS, decision, &mut out).expect("whole decision reserved");
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
    assert!(!journal.ready());
    assert!(root::takes(&journal, &LIMITS));
}
