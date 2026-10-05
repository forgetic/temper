use core::mem::size_of;
use root::loads::{self, Failure, Limits, Loads, Request};
use skein_lib::{Queue, Token, Wall};
use temper_engine_domain::{self as root, Key, Range, Record, TurnRecord};
use temper_engine_domain_world::commits::World;

const LIMITS: Limits = Limits { loads: 2, rows: 4, bytes: 1024, reply_bytes: 1024, transcript_bytes: 128 };

fn row(turn: u32, bytes: usize) -> Record {
    Record::Turn(TurnRecord {
        task: 1,
        attempt: 1,
        turn,
        spent: u64::from(turn),
        read: None,
        at: Wall::EPOCH,
        transcript: vec![b'x'; bytes].into_boxed_slice(),
    })
}

fn key(turn: u32) -> Key {
    Key::Turn { task: 1, attempt: 1, turn }
}

fn begin(loads: &mut Loads, after: Option<Key>, most: u32, out: &mut Queue<Request>) -> Token {
    let owner = loads::begin(loads, Token::new(51), Range::Turns { task: 1, attempt: 1 }, after, most, out)
        .expect("load admitted");
    assert_eq!(
        out.pop(),
        Some(Request::Load {
            owner,
            range: Range::Turns { task: 1, attempt: 1 },
            after,
            most,
            bytes: LIMITS.reply_bytes
        })
    );
    owner
}

#[test]
fn each_page_has_one_terminal_and_reads_the_answered_commit() {
    let mut world = World::new();
    for turn in 1..=4 {
        world.decision(turn, &[u8::try_from(turn).expect("four turns")], true, false);
        world.apply(true);
        world.ready();
    }
    let mut loads = Loads::new(&LIMITS);
    let mut out = Queue::with_capacity(1);
    let mut after = None;
    let mut observed = Vec::new();
    for expected_turns in [vec![1, 2], vec![3, 4]] {
        let owner = begin(&mut loads, after, 2, &mut out);
        let (rows, next) = world.store.page(Range::Turns { task: 1, attempt: 1 }, after, 2);
        let expected_rows = rows.clone();
        loads::loaded(&mut loads, owner, rows, next, &mut out);
        let Request::Loaded { waiter, rows, next: found, cut } = out.pop().expect("page terminal") else {
            panic!("loaded");
        };
        assert_eq!(waiter, Token::new(51));
        assert_eq!(rows, expected_rows, "durable store content");
        assert_eq!(cut, None);
        assert_eq!(found, next);
        assert_eq!(
            rows.iter()
                .map(|r| {
                    let Record::Turn(r) = r else {
                        panic!("turn");
                    };
                    r.turn
                })
                .collect::<Vec<_>>(),
            expected_turns
        );
        observed.extend(rows);
        loads::loaded(&mut loads, owner, Box::new([]), None, &mut out);
        loads::unloaded(&mut loads, owner, &mut out);
        assert!(out.is_empty(), "one terminal, including before reclaim");
        loads::reclaim(&mut loads);
        after = next;
    }
    assert_eq!(observed.len(), 4);
    assert_eq!(after, None);
    assert!(world.referee.done());
}

#[test]
fn abandoned_io_keeps_capacity_until_its_real_terminal_and_old_tokens_are_fenced() {
    let limits = Limits { loads: 1, ..LIMITS };
    let mut loads = Loads::new(&limits);
    let mut out = Queue::with_capacity(1);
    let old = begin(&mut loads, None, 1, &mut out);
    loads::abandon(&mut loads, old);
    loads::reclaim(&mut loads);
    assert!(loads::begin(&mut loads, Token::new(8), Range::Deployment, None, 1, &mut out).is_none());
    loads::loaded(&mut loads, old, vec![row(1, 4)].into_boxed_slice(), None, &mut out);
    assert!(out.is_empty());
    assert!(
        loads::begin(&mut loads, Token::new(8), Range::Deployment, None, 1, &mut out).is_none(),
        "retired until reclaim"
    );
    loads::reclaim(&mut loads);
    let new = begin(&mut loads, None, 1, &mut out);
    assert_ne!(old, new);
    loads::unloaded(&mut loads, old, &mut out);
    assert!(out.is_empty());
    loads::unloaded(&mut loads, new, &mut out);
    assert_eq!(out.pop(), Some(Request::Unloaded { waiter: Token::new(51), failure: Failure::Store }));
    loads::abandon(&mut loads, new);
    loads::unloaded(&mut loads, new, &mut out);
    assert!(out.is_empty());
}

#[test]
fn malformed_pages_are_refused_whole_before_any_row_is_delivered() {
    let cases = [
        (vec![row(1, 0), row(1, 0)], None, Failure::Order),
        (vec![row(2, 0), row(1, 0)], None, Failure::Order),
        (vec![Record::Deployment(temper_engine_domain_world::commits::HEADER)], None, Failure::Range),
        (vec![row(1, 0)], Some(key(2)), Failure::Cursor),
        (vec![], Some(key(1)), Failure::Cursor),
        ((1..=5).map(|turn| row(turn, 0)).collect(), None, Failure::Rows),
        (vec![row(1, 2048)], None, Failure::Bytes),
    ];
    for (rows, next, expected) in cases {
        let mut loads = Loads::new(&LIMITS);
        let mut out = Queue::with_capacity(1);
        let owner = begin(&mut loads, None, 4, &mut out);
        loads::loaded(&mut loads, owner, rows.into_boxed_slice(), next, &mut out);
        assert_eq!(out.pop(), Some(Request::Unloaded { waiter: Token::new(51), failure: expected }));
        loads::loaded(&mut loads, owner, vec![row(1, 0)].into_boxed_slice(), None, &mut out);
        assert!(out.is_empty());
    }
}

#[test]
fn byte_cut_keeps_the_whole_prefix_and_reports_exact_omissions_and_resume_cursor() {
    let slot = u32::try_from(size_of::<Record>()).expect("small fixed record");
    let limits = Limits { bytes: 2 * slot + 8, ..LIMITS };
    let mut loads = Loads::new(&limits);
    let mut out = Queue::with_capacity(1);
    let owner = begin(&mut loads, None, 4, &mut out);
    loads::loaded(
        &mut loads,
        owner,
        vec![row(1, 4), row(2, 4), row(3, 4), row(4, 4)].into_boxed_slice(),
        None,
        &mut out,
    );
    assert_eq!(
        out.pop(),
        Some(Request::Loaded {
            waiter: Token::new(51),
            rows: vec![row(1, 4), row(2, 4)].into_boxed_slice(),
            next: Some(key(2)),
            cut: Some(loads::Cut { rows: 2, bytes: 2 * (u64::from(slot) + 4) })
        })
    );
    loads::reclaim(&mut loads);
    let owner = begin(&mut loads, Some(key(2)), 2, &mut out);
    loads::loaded(&mut loads, owner, vec![row(3, 129), row(4, 0)].into_boxed_slice(), None, &mut out);
    assert_eq!(
        out.pop(),
        Some(Request::Loaded {
            waiter: Token::new(51),
            rows: Box::new([]),
            next: Some(key(2)),
            cut: Some(loads::Cut { rows: 2, bytes: 2 * u64::from(slot) + 129 })
        }),
        "an oversized first row is explicit nonprogress, never a false end of range"
    );
}

#[test]
fn store_terminals_are_taken_while_the_commit_barrier_has_no_admission_room() {
    let mut world = World::new();
    for turn in 1..=3 {
        world.decision(turn, b"pending", true, false);
    }
    assert!(!root::takes(&world.journal, &temper_engine_domain_world::commits::LIMITS));
    let mut loads = Loads::new(&LIMITS);
    let mut out = Queue::with_capacity(1);
    let owner = begin(&mut loads, None, 1, &mut out);
    loads::unloaded(&mut loads, owner, &mut out);
    assert_eq!(out.pop(), Some(Request::Unloaded { waiter: Token::new(51), failure: Failure::Store }));
    world.settle();
}
