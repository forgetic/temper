use core::mem::size_of;
use skein_lib::{Queue, Rng, Token, Wall};
use std::collections::BTreeSet;
use std::fmt::Write;
use temper_engine_domain::{
    Key, Range, Record, TurnRecord,
    loads::{self, Limits, Loads, Request},
};

fn run(seed: u64) -> (String, &'static str) {
    let mut rng = Rng::new(seed);
    let slot = u32::try_from(size_of::<Record>()).expect("small slot");
    let keep = u32::try_from(rng.below(4) + 1).expect("four rows");
    let limits =
        Limits { loads: 1, rows: 4, bytes: keep * (slot + 4), reply_bytes: 4 * (slot + 4), transcript_bytes: 4 };
    let mut domain = Loads::new(&limits);
    let mut out = Queue::with_capacity(1);
    let owner = loads::begin(&mut domain, Token::new(17), Range::Turns { task: 1, attempt: 1 }, None, 4, &mut out)
        .expect("slot");
    let mut trace = format!("{:?}", out.pop().expect("IO"));
    let ending;
    match seed % 4 {
        0 => {
            loads::unloaded(&mut domain, owner, &mut out);
            assert_eq!(out.pop(), Some(Request::Unloaded { waiter: Token::new(17), failure: loads::Failure::Store }));
            ending = "store failed";
        }
        1 => {
            loads::abandon(&mut domain, owner);
            loads::unloaded(&mut domain, owner, &mut out);
            assert!(out.is_empty());
            ending = "abandoned";
        }
        2 | 3 => {
            let rows: Box<[Record]> = (1..=4)
                .map(|turn| {
                    Record::Turn(TurnRecord {
                        task: 1,
                        attempt: 1,
                        turn,
                        spent: u64::from(turn),
                        read: None,
                        at: Wall::EPOCH,
                        transcript: vec![b'x'; 4].into_boxed_slice(),
                    })
                })
                .collect();
            loads::loaded(&mut domain, owner, rows, None, &mut out);
            let output = out.pop().expect("one page");
            write!(trace, "{output:?}").expect("trace string");
            let Request::Loaded { waiter, rows, next, cut } = output else {
                panic!("page");
            };
            assert_eq!(waiter, Token::new(17));
            assert_eq!(rows.len(), usize::try_from(keep).expect("small count"));
            for (at, row) in rows.iter().enumerate() {
                let Record::Turn(row) = row else {
                    panic!("turn");
                };
                assert_eq!(row.turn, u32::try_from(at + 1).expect("small turn"));
                assert_eq!(row.transcript.as_ref(), b"xxxx");
            }
            if keep == 4 {
                assert_eq!(next, None);
                assert_eq!(cut, None);
                ending = "whole";
            } else {
                assert_eq!(next, Some(Key::Turn { task: 1, attempt: 1, turn: keep }));
                assert_eq!(cut, Some(loads::Cut { rows: 4 - keep, bytes: u64::from((4 - keep) * (slot + 4)) }));
                ending = "cut";
            }
        }
        _ => unreachable!("modulo four"),
    }
    loads::unloaded(&mut domain, owner, &mut out);
    assert!(out.is_empty(), "duplicate terminal ignored");
    loads::reclaim(&mut domain);
    let new = loads::begin(&mut domain, Token::new(18), Range::Deployment, None, 1, &mut out).expect("next generation");
    assert_ne!(new, owner);
    drop(out.pop());
    loads::loaded(&mut domain, owner, Box::new([]), None, &mut out);
    assert!(out.is_empty(), "old page cannot complete replacement IO");
    (trace, ending)
}
#[test]
fn bounded_pages_and_terminal_races_replay_with_every_outcome() {
    let mut endings = BTreeSet::new();
    for seed in 0..64 {
        let first = run(seed);
        assert_eq!(first, run(seed), "seed {seed} replay");
        endings.insert(first.1);
    }
    assert_eq!(endings, BTreeSet::from(["whole", "cut", "store failed", "abandoned"]));
}
