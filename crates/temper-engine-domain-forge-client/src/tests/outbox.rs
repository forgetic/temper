use super::{LIMITS, REPO, Sent, at};
use crate::api::{self, Answer, Error, Op, Read, Write};
use crate::{
    Condition, Domain, Effect, Entry, Event, Limits, Made, Outcome, Recovery, RecoveryClock, Request, fire, max_out,
    recovery, resume, step,
};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Map, Queue, Wall};
#[derive(Debug)]
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    store: Map<u64, Entry>,
    outcomes: List<(u64, Outcome)>,
    calls: Queue<Sent>,
    writes: u32,
}
impl Harness {
    fn new() -> Harness {
        let mut h = Harness {
            domain: super::configured(&LIMITS),
            env: Env {
                now: at(100),
                wall: Wall::from_nanos(at(100).as_nanos()),
                limits: Limits { rate: 100, reserve: 0, ..LIMITS },
            },
            store: Map::with_capacity(LIMITS.entries),
            outcomes: List::with_capacity(20),
            calls: Queue::with_capacity(8),
            writes: 0,
        };
        h.step(Event::Restored { clock: RecoveryClock::Monotonic });
        h
    }
    fn step(&mut self, event: Event) {
        let mut out = Queue::with_capacity(max_out(&self.env.limits));
        step(&mut self.domain, &self.env, event, &mut out);
        self.take(&mut out);
        self.domain.reclaim();
    }
    fn take(&mut self, out: &mut Queue<Request>) {
        for _ in 0..out.len() {
            match out.pop().expect("output exists") {
                Request::Progress { entry } => {
                    self.store.insert(entry.number, entry).expect("store fits");
                }
                Request::Outcome { entry, task: _, outcome } => {
                    match outcome {
                        Outcome::Uncertain => {}
                        Outcome::Made { .. }
                        | Outcome::Failed(_)
                        | Outcome::Raced { .. }
                        | Outcome::Held
                        | Outcome::Withdrawn => {
                            self.store.remove(&entry).expect("top removes settled entry");
                        }
                    }
                    self.outcomes.push((entry, outcome)).expect("outcome fits");
                }
                Request::Call { call, repository, op } => {
                    assert_eq!(repository, REPO);
                    match &op {
                        Op::Write(write) => {
                            let mut durable = false;
                            for (_, entry) in &self.store {
                                if entry.effect.write == *write {
                                    let attempt = entry.attempt.expect("attempt durable before call starts");
                                    assert_eq!(attempt.sent, self.env.now);
                                    assert!(entry.start.is_some());
                                    durable = true;
                                }
                            }
                            assert!(durable, "a durable entry authorizes this exact write");
                            self.writes = self.writes.saturating_add(1);
                        }
                        Op::Read(_) => {}
                    }
                    self.calls.push(Sent { call, op });
                }
                Request::Save { .. }
                | Request::Erase { .. }
                | Request::Kept { .. }
                | Request::Changed { .. }
                | Request::Drift { .. } => panic!("outbox tests keep no resources"),
                Request::Read { .. } => panic!("outbox tests make no parent read requests"),
            }
        }
    }
    fn send(&mut self) -> Option<Sent> {
        let mut out = Queue::with_capacity(max_out(&self.env.limits));
        resume(&mut self.domain, &self.env, &mut out);
        self.take(&mut out);
        self.domain.reclaim();
        self.calls.pop()
    }
    fn one(&mut self) -> Sent {
        self.send().expect("call ready")
    }
    fn answer(&mut self, sent: Sent, result: Result<Answer, Error>) {
        self.step(Event::Answered { call: sent.call, cost: 1, result });
    }
    fn clock(&mut self) {
        let sent = self.one();
        assert_eq!(sent.op, Op::Read(Read::Items { since: at(u64::MAX), page: 1, kind: None }));
        self.answer(sent, Ok(Answer::Items { items: Box::new([]), more: false, now: at(100) }));
    }
    fn make(&mut self, number: u64, write: Write, condition: Condition) {
        let entry = Entry {
            number,
            task: 7,
            repository: REPO,
            effect: Effect { write, condition },
            start: None,
            attempt: None,
            failures: 0,
        };
        self.store.insert(number, entry.clone()).expect("top store fits");
        self.step(Event::Make { entry });
    }
    fn restart(&mut self, now: u64, wall: u64, clock: RecoveryClock) {
        self.domain = super::configured(&self.env.limits);
        self.env.now = at(now);
        self.env.wall = Wall::from_nanos(at(wall).as_nanos());
        let mut records = List::with_capacity(LIMITS.entries);
        for (_, entry) in &self.store {
            records.push(entry.clone()).expect("records fit");
        }
        self.step(Event::Restored { clock });
        for entry in records.into_boxed() {
            self.step(Event::Make { entry });
        }
    }
    fn fire(&mut self, now: u64) {
        self.env.now = at(now);
        let mut out = Queue::with_capacity(max_out(&self.env.limits));
        fire(&mut self.domain, &self.env, &mut out);
        self.take(&mut out);
        self.domain.reclaim();
    }
}
fn comment() -> Write {
    Write::Post { number: 9, key: Box::from(&b"deployment:task:milestone"[..]), body: Box::from(&b"done"[..]) }
}
#[test]
fn recovery_classes_follow_forgejo_guarantees() {
    assert_eq!(recovery(&comment()), Recovery::Unrecoverable);
    assert_eq!(
        recovery(&Write::CreateIssue { key: Box::from(&b"marker"[..]), title: Box::new([]), body: Box::new([]) }),
        Recovery::Unrecoverable,
    );
    assert_eq!(recovery(&Write::Update { number: 9 }), Recovery::Unrecoverable);
    assert_eq!(recovery(&Write::Merge { number: 9, head: [1; 32] }), Recovery::Conditional);
    assert_eq!(
        recovery(&Write::OpenPull {
            title: Box::new([]),
            body: Box::new([]),
            head: Box::from(&b"topic"[..]),
            base: Box::from(&b"main"[..]),
        }),
        Recovery::Keyed,
    );
    assert_eq!(recovery(&Write::Close { number: 9 }), Recovery::Idempotent);
}
fn summary() -> api::Summary {
    api::Summary {
        number: 9,
        kind: api::Kind::Issue,
        state: api::State::Open,
        author: 1,
        key: None,
        labels: Box::new([]),
        title: Box::new([]),
        body: Box::new([]),
        updated: at(100),
    }
}
fn pull(head: [u8; 32]) -> api::Pull {
    api::Pull {
        number: 9,
        state: api::State::Open,
        head: Box::from(&b"topic"[..]),
        base: Box::from(&b"main"[..]),
        commit: head,
        base_commit: Some([2; 32]),
        merged: None,
        mergeable: true,
        ci: api::Ci::Passed,
        reviewers: Box::new([]),
    }
}
#[test]
fn start_and_attempt_are_durable_before_the_first_write() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let sent = h.one();
    assert_eq!(sent.op, Op::Write(comment()));
    let entry = h.store.get(&1).unwrap();
    assert_eq!(entry.start.unwrap().at, at(100));
    assert_eq!(entry.attempt.unwrap().deadline, at(111));
    h.answer(sent, Ok(Answer::Commented(42)));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Made { made: Made::Commented(42), found: false })]);
    assert!(h.store.is_empty());
}
#[test]
fn an_uncertain_comment_is_found_by_key_without_a_second_write() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    let find = h.one();
    assert_eq!(find.op, Op::Read(Read::Item { number: 9, after: 0 }));
    let comment = api::Comment {
        provenance: api::Provenance::Original,
        id: 42,
        author: 1,
        created: at(100),
        revision: 1,
        key: Some(Box::from(&b"deployment:task:milestone"[..])),
        body: Box::new([]),
    };
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([comment]), more: false }));
    assert_eq!(h.writes, 1);
    assert_eq!(
        h.outcomes.as_slice(),
        &[(1, Outcome::Uncertain), (1, Outcome::Made { made: Made::Commented(42), found: true })]
    );
}
#[test]
fn restart_keeps_the_original_deadline_and_holds_an_unrecoverable_write() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    let original = h.store.get(&1).unwrap().clone();
    h.env.limits.lifetime = Duration::from_secs(100);
    h.restart(105, 50, RecoveryClock::Monotonic);
    let find = h.one();
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([]), more: false }));
    assert_eq!(h.domain.next_deadline(), Some(at(111)));
    assert_eq!(h.store.get(&1).unwrap(), &original);
    assert!(h.send().is_none());
    h.fire(111);
    let find = h.one();
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([]), more: false }));
    assert!(h.send().is_none());
    assert_eq!(h.writes, 1);
    assert_eq!(h.outcomes.as_slice().last(), Some(&(1, Outcome::Held)));
    assert!(h.store.is_empty());
}
#[test]
fn a_changed_monotonic_origin_uses_the_saved_absolute_expiry() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    h.restart(5, 105, RecoveryClock::Wall);
    let find = h.one();
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([]), more: false }));
    assert_eq!(h.domain.next_deadline(), Some(at(11)));
    assert_eq!(h.store.get(&1).unwrap().attempt.unwrap().expires.as_nanos(), at(111).as_nanos());
}
#[test]
fn lane_order_and_cancellation_hold_while_a_capture_call_is_out() {
    let mut h = Harness::new();
    h.make(1, Write::Close { number: 9 }, Condition::None);
    h.make(2, Write::Reopen { number: 9 }, Condition::None);
    let clock = h.one();
    assert!(h.send().is_none(), "later lane entry waits");
    h.step(Event::Withdraw { entry: 1 });
    h.answer(clock, Ok(Answer::Items { items: Box::new([]), more: false, now: at(100) }));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Withdrawn)]);
    h.clock();
    let write = h.one();
    assert_eq!(write.op, Op::Write(Write::Reopen { number: 9 }));
    h.answer(write, Ok(Answer::Done));
    assert_eq!(h.writes, 1);
}
#[test]
fn a_retargeted_merge_fails_before_any_write() {
    let mut h = Harness::new();
    h.make(1, Write::Merge { number: 9, head: [1; 32] }, Condition::Merge { base: Box::from(&b"other"[..]) });
    h.clock();
    let check = h.one();
    assert_eq!(check.op, Op::Read(Read::Pull { number: 9 }));
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    assert_eq!(h.writes, 0);
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Failed(Error::Stale))]);
}
#[test]
fn update_recovery_proves_the_requested_base_is_an_ancestor() {
    let mut h = Harness::new();
    h.make(1, Write::Update { number: 9 }, Condition::Update { head: [1; 32], base: [2; 32] });
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let update = h.one();
    h.answer(update, Err(Error::Timeout));
    let find = h.one();
    h.answer(find, Ok(Answer::Pull(pull([3; 32]))));
    let compare = h.one();
    assert_eq!(compare.op, Op::Read(Read::Compare { before: [3; 32], after: [2; 32] }));
    h.answer(
        compare,
        Ok(Answer::Compare {
            before: [3; 32],
            after: [2; 32],
            contains_before: false,
            files: Box::new([]),
            commits: Box::new([]),
        }),
    );
    assert_eq!(h.writes, 1);
    assert_eq!(h.outcomes.as_slice().last(), Some(&(1, Outcome::Made { made: Made::Updated([3; 32]), found: true })));
}
#[test]
fn accepted_merge_requires_post_result_base_and_head_verification() {
    let mut h = Harness::new();
    h.make(1, Write::Merge { number: 9, head: [1; 32] }, Condition::Merge { base: Box::from(&b"main"[..]) });
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let merge = h.one();
    h.answer(merge, Ok(Answer::Merged([4; 32])));
    assert!(h.outcomes.is_empty());
    let verify = h.one();
    let mut current = pull([1; 32]);
    current.merged = Some([4; 32]);
    current.state = api::State::Closed;
    h.answer(verify, Ok(Answer::Pull(current)));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Made { made: Made::Merged([4; 32]), found: false })]);
}
#[test]
fn a_retarget_between_precheck_and_merge_is_explicitly_a_raced_effect() {
    let mut h = Harness::new();
    h.make(1, Write::Merge { number: 9, head: [1; 32] }, Condition::Merge { base: Box::from(&b"main"[..]) });
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let merge = h.one();
    h.answer(merge, Ok(Answer::Merged([4; 32])));
    let verify = h.one();
    let mut current = pull([1; 32]);
    current.base = Box::from(&b"other"[..]);
    current.merged = Some([4; 32]);
    h.answer(verify, Ok(Answer::Pull(current)));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Raced { made: Made::Merged([4; 32]), why: Error::Stale })]);
    assert_eq!(h.writes, 1);
}
#[test]
fn accepted_update_cannot_be_reported_as_failed_after_read_faults() {
    let mut h = Harness::new();
    h.make(1, Write::Update { number: 9 }, Condition::Update { head: [1; 32], base: [2; 32] });
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let update = h.one();
    h.answer(update, Ok(Answer::Done));
    let verify = h.one();
    h.answer(verify, Err(Error::Forbidden));
    assert!(h.outcomes.is_empty());
    assert!(h.store.get(&1).is_some());
    assert!(h.domain.next_deadline().is_some());
}
#[test]
fn a_rehanded_entry_with_a_started_attempt_finds_before_writing() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    let record = h.store.get(&1).unwrap().clone();
    h.domain = super::configured(&h.env.limits);
    h.env.now = at(105);
    h.step(Event::Restored { clock: RecoveryClock::Monotonic });
    h.step(Event::Make { entry: record.clone() });
    let find = h.one();
    assert_eq!(find.op, Op::Read(Read::Item { number: 9, after: 0 }));
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([]), more: false }));
    assert_eq!(h.domain.next_deadline(), Some(record.attempt.unwrap().deadline));
    assert_eq!(h.writes, 1);
}
#[test]
fn an_unavailable_write_retries_only_up_to_its_configured_bound() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    for attempt in 1..=LIMITS.write_attempts {
        let sent = h.one();
        assert_eq!(sent.op, Op::Write(comment()));
        h.answer(sent, Err(Error::Unavailable));
        if attempt < LIMITS.write_attempts {
            let due = h.domain.next_deadline().unwrap();
            h.env.now = due;
            let mut out = Queue::with_capacity(max_out(&h.env.limits));
            fire(&mut h.domain, &h.env, &mut out);
            h.take(&mut out);
        }
    }
    assert_eq!(h.writes, LIMITS.write_attempts);
    assert_eq!(h.outcomes.as_slice().last(), Some(&(1, Outcome::Failed(Error::Unavailable))));
}
#[test]
fn an_uncertain_set_waits_for_its_original_lifetime_without_neighbor_probes() {
    let mut h = Harness::new();
    h.make(1, Write::Close { number: 9 }, Condition::None);
    h.clock();
    let write = h.one();
    h.answer(write, Err(Error::Timeout));
    assert!(h.send().is_none());
    assert_eq!(h.domain.next_deadline(), Some(at(111)));
    assert_eq!(h.writes, 1);
    h.fire(111);
    assert!(h.send().is_none());
    let write = h.one();
    assert_eq!(write.op, Op::Write(Write::Close { number: 9 }));
    h.answer(write, Ok(Answer::Done));
    assert_eq!(h.writes, 2);
}
#[test]
fn comparison_overflow_leaves_update_uncertain_without_another_write() {
    let mut h = Harness::new();
    h.make(1, Write::Update { number: 9 }, Condition::Update { head: [1; 32], base: [2; 32] });
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = h.one();
    h.answer(write, Err(Error::Timeout));
    let find = h.one();
    h.answer(find, Ok(Answer::Pull(pull([3; 32]))));
    let compare = h.one();
    h.answer(compare, Err(Error::TooLarge));
    assert_eq!(h.writes, 1);
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
    assert!(h.store.get(&1).is_some());
    assert!(h.domain.next_deadline().is_some());
}
#[test]
fn a_review_written_after_the_head_moves_is_classified_as_a_raced_effect() {
    let mut h = Harness::new();
    h.make(
        1,
        Write::Review {
            number: 9,
            key: Box::from(&b"review-key"[..]),
            verdict: api::Verdict::Approve,
            body: Box::from(&b"looks good"[..]),
        },
        Condition::Review { head: [1; 32] },
    );
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = h.one();
    h.answer(write, Ok(Answer::Reviewed(7)));
    let find = h.one();
    let review = api::Review {
        provenance: api::Provenance::Original,
        id: 7,
        revision: 1,
        author: 1,
        verdict: api::Verdict::Approve,
        commit: [3; 32],
        key: Some(Box::from(&b"review-key"[..])),
        body: Box::new([]),
        at: at(100),
        official: true,
    };
    h.answer(find, Ok(Answer::Reviews { reviews: Box::new([review]), more: false }));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Raced { made: Made::Reviewed(7), why: Error::Stale })]);
}
#[test]
fn a_created_branch_is_found_only_at_its_requested_commit() {
    let mut h = Harness::new();
    h.make(1, Write::CreateBranch { branch: Box::from(&b"topic"[..]), commit: [1; 32] }, Condition::None);
    h.clock();
    let write = h.one();
    h.answer(write, Ok(Answer::Branch(api::BranchCreation::Exists)));
    let find = h.one();
    h.answer(find, Ok(Answer::Commit([2; 32])));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Failed(Error::Exists))]);
}
#[test]
fn a_copied_pending_key_cannot_establish_the_effects_outcome() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    let find = h.one();
    let copied = api::Comment {
        provenance: api::Provenance::Original,
        id: 41,
        author: 2,
        created: at(100),
        revision: 1,
        key: Some(Box::from(&b"deployment:task:milestone"[..])),
        body: Box::new([]),
    };
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([copied]), more: true }));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
    let find = h.one();
    assert_eq!(find.op, Op::Read(Read::Item { number: 9, after: 41 }));
    let own = api::Comment {
        provenance: api::Provenance::Original,
        id: 42,
        author: 1,
        created: at(100),
        revision: 1,
        key: Some(Box::from(&b"deployment:task:milestone"[..])),
        body: Box::new([]),
    };
    h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([own]), more: false }));
    assert_eq!(h.writes, 1);
    assert_eq!(
        h.outcomes.as_slice(),
        &[(1, Outcome::Uncertain), (1, Outcome::Made { made: Made::Commented(42), found: true })]
    );
}
#[test]
fn revised_or_unknown_comments_do_not_settle_an_uncertain_creation() {
    for provenance in [api::Provenance::Revised, api::Provenance::Unknown] {
        let mut h = Harness::new();
        h.make(1, comment(), Condition::None);
        h.clock();
        let write = h.one();
        h.answer(write, Err(Error::Timeout));
        h.restart(105, 105, RecoveryClock::Monotonic);
        let find = h.one();
        let row = api::Comment {
            provenance,
            id: 42,
            author: 1,
            created: at(100),
            revision: 2,
            key: Some(Box::from(&b"deployment:task:milestone"[..])),
            body: Box::new([]),
        };
        h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([row]), more: false }));
        assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
        assert_eq!(h.writes, 1);
        assert_eq!(h.domain.next_deadline(), Some(at(111)));
        assert!(h.send().is_none());
        h.fire(111);
        let find = h.one();
        let original = api::Comment {
            provenance: api::Provenance::Original,
            id: 43,
            author: 1,
            created: at(100),
            revision: 1,
            key: Some(Box::from(&b"deployment:task:milestone"[..])),
            body: Box::new([]),
        };
        h.answer(find, Ok(Answer::Item { item: summary(), comments: Box::new([original]), more: false }));
        assert_eq!(h.writes, 1);
        assert_eq!(
            h.outcomes.as_slice(),
            &[(1, Outcome::Uncertain), (1, Outcome::Made { made: Made::Commented(43), found: true }),]
        );
    }
}
fn recovering_review() -> Harness {
    let mut h = Harness::new();
    h.make(
        1,
        Write::Review {
            number: 9,
            key: Box::from(&b"review-key"[..]),
            verdict: api::Verdict::Approve,
            body: Box::from(&b"looks good"[..]),
        },
        Condition::Review { head: [1; 32] },
    );
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = h.one();
    h.answer(write, Err(Error::Timeout));
    // Restore the durable boundary a participating inbox captured before
    // submission. The outbox-only harness does not keep a live resource.
    h.store.get_mut(&1).expect("uncertain entry remains").start.as_mut().expect("durable start").review = 5;
    h.restart(105, 105, RecoveryClock::Monotonic);
    h
}
fn recovered_review(id: u64, provenance: api::Provenance, commit: [u8; 32]) -> api::Review {
    api::Review {
        provenance,
        id,
        revision: 1,
        author: 1,
        verdict: api::Verdict::Approve,
        commit,
        key: Some(Box::from(&b"review-key"[..])),
        body: Box::new([]),
        at: at(100),
        official: true,
    }
}
#[test]
fn review_recovery_rejects_revised_or_unknown_rows_before_classifying_the_head() {
    for provenance in [api::Provenance::Revised, api::Provenance::Unknown] {
        for head in [[1; 32], [3; 32]] {
            let mut h = recovering_review();
            let find = h.one();
            h.answer(
                find,
                Ok(Answer::Reviews { reviews: Box::new([recovered_review(6, provenance, head)]), more: false }),
            );
            assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
            assert_eq!(h.writes, 1);
            assert_eq!(h.store.get(&1).expect("unknown entry remains").start.expect("durable boundary").review, 5);
            assert_eq!(h.domain.next_deadline(), Some(at(111)));
            assert!(h.send().is_none());
        }
    }
}
#[test]
fn review_recovery_requires_an_original_row_strictly_after_the_saved_boundary() {
    for head in [[1; 32], [3; 32]] {
        let mut h = recovering_review();
        let find = h.one();
        assert_eq!(find.op, Op::Read(Read::Reviews { number: 9, page: 1 }));
        h.answer(
            find,
            Ok(Answer::Reviews {
                reviews: Box::new([
                    recovered_review(4, api::Provenance::Original, head),
                    recovered_review(5, api::Provenance::Original, head),
                ]),
                more: true,
            }),
        );
        assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
        let find = h.one();
        assert_eq!(find.op, Op::Read(Read::Reviews { number: 9, page: 2 }));
        h.answer(
            find,
            Ok(Answer::Reviews {
                reviews: Box::new([recovered_review(6, api::Provenance::Original, [1; 32])]),
                more: false,
            }),
        );
        assert_eq!(h.writes, 1);
        assert_eq!(
            h.outcomes.as_slice(),
            &[(1, Outcome::Uncertain), (1, Outcome::Made { made: Made::Reviewed(6), found: true }),]
        );
    }
}
#[test]
fn effect_admission_and_restoration_require_authenticated_writer_configuration() {
    let mut h = Harness::new();
    h.domain = Domain::new(&h.env.limits);
    h.step(Event::Restored { clock: RecoveryClock::Monotonic });
    h.make(1, comment(), Condition::None);
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Failed(Error::Forbidden))]);
    assert!(h.send().is_none());
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    let record = h.store.get(&1).expect("saved entry").clone();
    h.domain = Domain::new(&h.env.limits);
    h.step(Event::Restored { clock: RecoveryClock::Monotonic });
    h.step(Event::Make { entry: record });
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Failed(Error::Forbidden))]);
    assert!(h.send().is_none());
}
