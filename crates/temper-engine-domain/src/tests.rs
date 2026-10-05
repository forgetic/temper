use crate::decision::{
    Decision, Delivery, Journal, Limits, Output, accept, committed, fresh, resume, takes, uncommitted,
};
use crate::{Deployment, Family, Key, Record, TurnRecord, Write};
use alloc::boxed::Box;
use skein_lib::{List, Queue, Wall};

const LIMITS: Limits = Limits { commits: 2, held: 8, writes: 4, deliveries: 4, transcript_bytes: 64, result_bytes: 32 };
const DEPLOYMENT: Deployment =
    Deployment { id: [19; 16], tasks: 0, people: 0, sign_ins: 0, messages: 0, runs: 0, calls: 0, commits: 0 };
fn bytes(len: u32) -> Box<[u8]> {
    let mut bytes = List::with_capacity(len);
    for _ in 0..len {
        bytes.push(b'x').unwrap();
    }
    bytes.into_boxed()
}
fn turn(number: u32, len: u32) -> Write {
    Write::Save(Record::Turn(TurnRecord {
        task: 1,
        attempt: 1,
        turn: number,
        spent: u64::from(number),
        read: Some(u64::from(number)),
        at: Wall::from_nanos(17),
        transcript: bytes(len),
    }))
}
fn ack(number: u32) -> Delivery {
    Delivery::AcknowledgeTurn { channel: 7, task: 1, attempt: 1, turn: number }
}
fn decision(number: u32) -> Decision {
    let mut d = Decision::new(&LIMITS);
    d.write(&LIMITS, turn(number, 2)).unwrap();
    d.deliver(&LIMITS, ack(number)).unwrap();
    d
}
fn commit(out: &mut Queue<Output>) -> (u64, Box<[Write]>) {
    let Output::Commit { number, writes } = out.pop().unwrap() else {
        panic!("commit");
    };
    (number, writes)
}
fn acknowledged(out: &mut Queue<Output>) -> u32 {
    let Output::Deliver(Delivery::AcknowledgeTurn { turn, .. }) = out.pop().unwrap() else {
        panic!("ack");
    };
    turn
}
#[test]
fn a_store_answer_makes_outputs_ready_without_draining_them() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    accept(&mut j, &LIMITS, decision(1), &mut out).unwrap();
    let (number, writes) = commit(&mut out);
    assert_eq!(number, 1);
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0], Write::Save(Record::Deployment(Deployment { commits: 1, ..DEPLOYMENT })));
    resume(&mut j, &mut out);
    assert!(out.is_empty());
    committed(&mut j, number);
    assert!(out.is_empty());
    assert!(j.ready());
    resume(&mut j, &mut out);
    assert_eq!(acknowledged(&mut out), 1);
    resume(&mut j, &mut out);
    assert!(out.is_empty());
}
#[test]
fn a_decision_without_writes_still_waits_for_the_state_it_observed() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    accept(&mut j, &LIMITS, decision(1), &mut out).unwrap();
    drop(commit(&mut out));
    let mut read = Decision::new(&LIMITS);
    read.deliver(&LIMITS, ack(2)).unwrap();
    accept(&mut j, &LIMITS, read, &mut out).unwrap();
    assert!(out.is_empty());
    resume(&mut j, &mut out);
    assert!(out.is_empty());
    committed(&mut j, 1);
    resume(&mut j, &mut out);
    assert_eq!(acknowledged(&mut out), 1);
    resume(&mut j, &mut out);
    assert_eq!(acknowledged(&mut out), 2);
    assert_eq!(j.deployment().commits, 1);
}
#[test]
fn cumulative_and_duplicate_store_answers_preserve_original_output_order() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    for turn in 1_u32..=2 {
        accept(&mut j, &LIMITS, decision(turn), &mut out).unwrap();
        assert_eq!(commit(&mut out).0, u64::from(turn));
    }
    assert!(!takes(&j, &LIMITS));
    committed(&mut j, 2);
    committed(&mut j, 1);
    committed(&mut j, 2);
    assert_eq!(j.durable(), 2);
    for turn in 1_u32..=2 {
        resume(&mut j, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(acknowledged(&mut out), turn);
    }
    assert!(takes(&j, &LIMITS));
}
#[test]
fn a_failed_commit_releases_neither_its_outputs_nor_later_outputs() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    for turn in 1_u32..=2 {
        accept(&mut j, &LIMITS, decision(turn), &mut out).unwrap();
        drop(commit(&mut out));
    }
    uncommitted(&mut j, 1, &mut out);
    assert_eq!(out.pop(), Some(Output::Stop));
    committed(&mut j, 2);
    uncommitted(&mut j, 2, &mut out);
    resume(&mut j, &mut out);
    assert!(out.is_empty());
    assert!(j.stopped());
    assert!(!takes(&j, &LIMITS));
    assert_eq!(fresh(&mut j, Family::Task), None);
}
#[test]
fn held_output_pressure_is_seen_before_the_next_child_decision() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    for _ in 0_u32..2 {
        let mut d = Decision::new(&LIMITS);
        for turn in 1_u32..=4 {
            d.deliver(&LIMITS, ack(turn)).unwrap();
        }
        accept(&mut j, &LIMITS, d, &mut out).unwrap();
    }
    assert!(!takes(&j, &LIMITS));
    let before = j.deployment();
    let returned = accept(&mut j, &LIMITS, decision(1), &mut out).unwrap_err();
    assert_eq!(j.deployment(), before);
    assert!(out.is_empty());
    for _ in 0_u32..4 {
        resume(&mut j, &mut out);
        drop(out.pop());
    }
    assert!(takes(&j, &LIMITS));
    accept(&mut j, &LIMITS, returned, &mut out).unwrap();
    let (_, writes) = commit(&mut out);
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[1], turn(1, 2));
}
#[test]
fn fresh_numbers_and_unused_gaps_are_durable_in_the_decisions_header() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    for family in [Family::Task, Family::Person, Family::SignIn, Family::Message, Family::Run, Family::Call] {
        assert_eq!(fresh(&mut j, family), Some(1));
    }
    assert_eq!(fresh(&mut j, Family::Task), Some(2));
    accept(&mut j, &LIMITS, Decision::new(&LIMITS), &mut out).unwrap();
    let (number, writes) = commit(&mut out);
    assert_eq!(number, 1);
    let Write::Save(Record::Deployment(header)) = writes[0] else {
        panic!("header");
    };
    assert_eq!(header.tasks, 2);
    let mut restarted = Journal::new(header, &LIMITS);
    assert_eq!(restarted.durable(), 1);
    assert_eq!(fresh(&mut restarted, Family::Task), Some(3));
}
#[test]
fn same_key_saves_and_erases_collapse_to_the_last_write_before_commit() {
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    let mut d = Decision::new(&LIMITS);
    d.write(&LIMITS, turn(1, 2)).unwrap();
    d.write(&LIMITS, turn(2, 3)).unwrap();
    d.write(&LIMITS, Write::Erase(Key::Turn { task: 1, attempt: 1, turn: 1 })).unwrap();
    d.write(&LIMITS, turn(1, 4)).unwrap();
    accept(&mut j, &LIMITS, d, &mut out).unwrap();
    let (_, writes) = commit(&mut out);
    assert_eq!(writes.len(), 3);
    assert_eq!(writes[1], turn(1, 4));
    assert_eq!(writes[2], turn(2, 3));
}
#[test]
fn rejected_payloads_are_returned_without_replacing_admitted_ownership() {
    let mut d = Decision::new(&LIMITS);
    d.write(&LIMITS, turn(1, 3)).unwrap();
    let refused = d.write(&LIMITS, turn(1, 65)).unwrap_err();
    assert_eq!(refused, turn(1, 65));
    let refused = d.deliver(&LIMITS, Delivery::Result { person: 1, task: 1, words: bytes(33) }).unwrap_err();
    let Delivery::Result { words, .. } = refused else {
        panic!("result");
    };
    assert_eq!(words.len(), 33);
    assert!(d.write(&LIMITS, Write::Erase(Key::Deployment)).is_err());
    let mut j = Journal::new(DEPLOYMENT, &LIMITS);
    let mut out = Queue::with_capacity(1);
    accept(&mut j, &LIMITS, d, &mut out).unwrap();
    let (_, writes) = commit(&mut out);
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[1], turn(1, 3));
    committed(&mut j, 1);
    resume(&mut j, &mut out);
    assert!(out.is_empty());
}
#[test]
fn counter_and_commit_overflow_cannot_reuse_names() {
    let mut j = Journal::new(Deployment { tasks: u64::MAX, commits: u64::MAX, ..DEPLOYMENT }, &LIMITS);
    assert_eq!(fresh(&mut j, Family::Task), None);
    assert!(!takes(&j, &LIMITS));
    assert_eq!(j.deployment().tasks, u64::MAX);
}
