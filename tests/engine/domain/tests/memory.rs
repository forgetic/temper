use skein_lib::{Queue, Wall};
use temper_engine_domain::{self as root, Decision, Delivery, Journal, Output, Record, TurnRecord, Write};
use temper_engine_domain_world::commits::{HEADER, LIMITS};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

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
