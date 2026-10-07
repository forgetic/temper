//! Unit tests exercise the boundary and the requests it emits.

use alloc::boxed::Box;

use skein_lib::{Env, List, Queue, Time, Token, Wall};

use crate::{
    Author, Change, Domain, Entry, Event, Limits, New, Range, Record, Refusal, Request, Rows, Scope, max_out, step,
    worst_case,
};

const LIMITS: Limits = Limits {
    entries_per_scope: 1,
    pattern_bytes: 16,
    description_bytes: 16,
    body_bytes: 32,
    references: 2,
    load_rows: 2,
};

fn new(name: u64, description: &[u8]) -> New {
    New {
        name,
        scope: Scope::Project { project: 7 },
        description: Box::from(description),
        body: Box::from(&b"body"[..]),
        references: List::with_capacity(2),
        author: Author::Task { task: 9, attempt: 1 },
    }
}

fn rows(records: &[Record]) -> Rows {
    let mut list = List::with_capacity(2);
    for record in records {
        list.push(record.clone()).expect("test page fits");
    }
    Rows { records: list }
}

struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new() -> Harness {
        Harness {
            domain: Domain::new(&LIMITS),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS },
            out: Queue::with_capacity(max_out(&LIMITS)),
        }
    }

    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        let mut got = List::with_capacity(self.out.len());
        while let Some(request) = self.out.pop() {
            got.push(request).expect("test output fits");
        }
        got.into_boxed()
    }
}

#[test]
fn a_new_entry_is_saved_with_its_scope_line_in_one_decision() {
    let mut h = Harness::new();
    let asked = h.step(Event::Write { owner: Token::new(4), entry: new(20, b"warmup"), recalled: None });
    let [Request::Load { owner, range: Range::Entry { name: 20 } }] = &*asked else {
        panic!("entry lookup: {asked:?}");
    };
    let owner = *owner;
    let asked = h.step(Event::Loaded { owner, rows: rows(&[]), more: false });
    let [Request::Load { range: Range::Lines { .. }, .. }] = &*asked else {
        panic!("scope lookup: {asked:?}");
    };
    let got = h.step(Event::Loaded { owner, rows: rows(&[]), more: false });
    let [
        Request::Save { record: Record::Entry(_) },
        Request::Save { record: Record::Line(_) },
        Request::Written { owner, name: 20, revision: 1 },
    ] = &*got
    else {
        panic!("new entry writes: {got:?}");
    };
    assert_eq!(*owner, Token::new(4));
    assert!(!h.domain.busy());
}

#[test]
fn a_revision_must_name_the_entry_the_writer_recalled() {
    let mut h = Harness::new();
    let asked = h.step(Event::Write { owner: Token::new(5), entry: new(20, b"revised"), recalled: Some(1) });
    let [Request::Load { owner, .. }] = &*asked else {
        panic!("entry lookup");
    };
    let old = Entry {
        name: 20,
        scope: Scope::Project { project: 7 },
        description: Box::from(&b"old"[..]),
        body: Box::from(&b"body"[..]),
        references: List::with_capacity(2),
        author: Author::Party { party: 2 },
        revision: 2,
    };
    let got = h.step(Event::Loaded { owner: *owner, rows: rows(&[Record::Entry(old)]), more: false });
    assert_eq!(&*got, &[Request::Refused { owner: Token::new(5), why: Refusal::Moved }]);
}

#[test]
fn a_party_deletes_only_the_revision_it_saw() {
    let mut h = Harness::new();
    let asked =
        h.step(Event::Edit { owner: Token::new(6), party: 2, name: 20, change: Change::Delete { recalled: 2 } });
    let [Request::Load { owner, .. }] = &*asked else {
        panic!("entry lookup");
    };
    let old = Entry {
        name: 20,
        scope: Scope::Project { project: 7 },
        description: Box::from(&b"old"[..]),
        body: Box::from(&b"body"[..]),
        references: List::with_capacity(2),
        author: Author::Task { task: 1, attempt: 1 },
        revision: 2,
    };
    let got = h.step(Event::Loaded { owner: *owner, rows: rows(&[Record::Entry(old)]), more: false });
    let [Request::Erase { .. }, Request::Erase { .. }, Request::Deleted { owner, name: 20 }] = &*got else {
        panic!("delete writes: {got:?}");
    };
    assert_eq!(*owner, Token::new(6));
}

#[test]
fn a_full_scope_refuses_a_new_entry() {
    let mut h = Harness::new();
    let asked = h.step(Event::Write { owner: Token::new(7), entry: new(21, b"new"), recalled: None });
    let [Request::Load { owner, .. }] = &*asked else {
        panic!("entry lookup");
    };
    let owner = *owner;
    let _asked = h.step(Event::Loaded { owner, rows: rows(&[]), more: false });
    let line = crate::Line {
        scope: Scope::Project { project: 7 },
        name: 20,
        description: Box::from(&b"old"[..]),
        revision: 1,
    };
    let got = h.step(Event::Loaded { owner, rows: rows(&[Record::Line(line)]), more: false });
    assert_eq!(&*got, &[Request::Refused { owner: Token::new(7), why: Refusal::Full }]);
}

#[test]
fn the_memory_bound_accepts_the_test_limits() {
    assert!(worst_case(&LIMITS).is_some());
}
