//! First-slice task, escalation and result steps.
use crate::*;
use alloc::boxed::Box;
use skein_lib::{Env, Id, Queue, Time, Token, Wall};

fn limits() -> Limits {
    let mut limits = tests::limits();
    limits.streams = 2;
    limits.objects = 3;
    limits.notices = 8;
    limits
}

fn env(now: u64) -> Env<Limits> {
    Env { now: Time::from_nanos(now), wall: Wall::from_nanos(now), limits: limits() }
}

fn pop(out: &mut Queue<Request>) -> Request {
    out.pop().expect("expected output")
}

fn person() -> PersonSnapshot {
    PersonSnapshot {
        person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
        projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
        inbox_count: 1,
    }
}

fn chip(revision: u64) -> Chip {
    Chip {
        task: 42,
        project: 7,
        title: Box::from(b"Fix login".as_slice()),
        phase: TaskPhase::Held(HoldReason::Tries),
        spent: 3,
        budget: 12,
        revision,
    }
}

fn escalation(revision: u64) -> Escalation {
    Escalation {
        task: 42,
        reason: HoldReason::Tries,
        waiting_since: Wall::EPOCH,
        offers: Offers { release: true, leave_held: true, pass_up: false },
        revision,
    }
}

fn snapshot(revision: u64) -> TaskSnapshot {
    TaskSnapshot {
        chip: chip(revision),
        first_words: Box::from(b"Fix login".as_slice()),
        escalation: Some(escalation(revision)),
        result: None,
    }
}

fn task_started(seed: u64) -> (Domain, Queue<Request>, Token, Token, Id<Object>) {
    let limits = limits();
    let mut domain = Domain::new(&limits, seed);
    let mut out = Queue::with_capacity(max_out(&limits));
    step(
        &mut domain,
        &env(0),
        Event::Start { address: Address::Task { number: 42, section: None }, saved: None, offset: Offset(0) },
        &mut out,
    );
    let Request::Open { stream: person_stream, watch: Watch::Person } = pop(&mut out) else {
        panic!("person watch opens")
    };
    let Request::Open { stream: task_stream, watch: Watch::Task { number: 42 } } = pop(&mut out) else {
        panic!("task watch opens")
    };
    step(&mut domain, &env(0), Event::Opened { stream: person_stream }, &mut out);
    step(
        &mut domain,
        &env(0),
        Event::Streamed { stream: person_stream, event: StreamEvent::Snapshot(Snapshot::Person(person())) },
        &mut out,
    );
    step(&mut domain, &env(0), Event::Opened { stream: task_stream }, &mut out);
    step(
        &mut domain,
        &env(0),
        Event::Streamed { stream: task_stream, event: StreamEvent::Snapshot(Snapshot::Task(snapshot(1))) },
        &mut out,
    );
    assert!(out.is_empty(), "snapshots have no shell output");
    let Page::Task(page) = domain.page() else { panic!("task page opens") };
    assert_eq!(page.first_words.as_deref(), Some(b"Fix login".as_slice()), "opening words shown");
    let id = page.escalation.expect("escalation card is on page");
    (domain, out, person_stream, task_stream, id)
}

#[test]
fn release_is_saved_then_sent_and_lingers_after_answer() {
    let (mut domain, mut out, _, _, object) = task_started(201);
    step(&mut domain, &env(0), Event::Act { action: Action::Intend { intent: Intent::Release, object } }, &mut out);
    assert_eq!(domain.confirming().expect("confirmation open").revision, 1, "intent remembers shown revision");
    step(&mut domain, &env(0), Event::Act { action: Action::Confirm }, &mut out);
    let Request::Save { saved } = pop(&mut out) else { panic!("key saved before send") };
    assert_eq!(saved.pending.len(), 1, "decision is kept");
    let Request::Send {
        request,
        ask: Ask::Decide { waiting: Waiting::Escalation { task: 42 }, revision: 1, decision: Decision::Release },
        ..
    } = pop(&mut out)
    else {
        panic!("release sent")
    };
    let Card::Deciding { request: shown } = &domain.object(object).expect("card retained").card else {
        panic!("card is deciding")
    };
    assert_eq!(*shown, request, "card names pending request");
    step(
        &mut domain,
        &env(0),
        Event::Answered { request, answer: Answer::Done(Outcome::Decided { choice: Choice::Released }) },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("durable answer saved") };
    assert!(saved.pending.is_empty(), "answered key retired");
    let Card::Leaving { why: Why::Decided { by, choice: Choice::Released, .. }, .. } =
        &domain.object(object).expect("card lingers").card
    else {
        panic!("decision lingers")
    };
    assert_eq!(by.number, 1, "person is shown as decider");
    let at = domain.next_deadline().expect("linger timer armed");
    fire(&mut domain, &env(at.as_nanos()), &mut out);
    assert!(domain.object(object).is_none(), "card leaves after linger");
}

#[test]
fn leaving_held_requires_reason_and_edit_saves_it() {
    let (mut domain, mut out, _, _, object) = task_started(202);
    step(&mut domain, &env(0), Event::Act { action: Action::Intend { intent: Intent::LeaveHeld, object } }, &mut out);
    step(&mut domain, &env(0), Event::Act { action: Action::Confirm }, &mut out);
    assert_eq!(
        domain.confirming().expect("confirmation stays").problem,
        Some(Problem::ReasonMissing),
        "reason required"
    );
    assert!(out.is_empty(), "missing reason sends nothing");
    step(
        &mut domain,
        &env(0),
        Event::Act {
            action: Action::Edit { field: FieldRef::Reason, text: Box::from(b"Need a larger budget".as_slice()) },
        },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("reason edit saves immediately") };
    assert_eq!(saved.drafts.get(1).expect("reason saved").text.as_ref(), b"Need a larger budget", "reason is in Saved");
    step(&mut domain, &env(0), Event::Act { action: Action::Confirm }, &mut out);
    let Request::Save { .. } = pop(&mut out) else { panic!("decision saved") };
    let Request::Send { ask: Ask::Decide { decision: Decision::Reject { reason }, .. }, .. } = pop(&mut out) else {
        panic!("rejection sent")
    };
    assert_eq!(reason.as_ref(), b"Need a larger budget", "engine receives exact reason");
    assert!(domain.confirming().is_none(), "confirmation closes after send");
    assert!(domain.field(FieldRef::Reason).expect("field exists").text.is_empty(), "reason cleared after send");
}

#[test]
fn revision_change_refuses_confirmation_without_sending() {
    let (mut domain, mut out, _, task_stream, object) = task_started(203);
    step(&mut domain, &env(0), Event::Act { action: Action::Intend { intent: Intent::Release, object } }, &mut out);
    step(
        &mut domain,
        &env(0),
        Event::Streamed { stream: task_stream, event: StreamEvent::Change(Change::Task(snapshot(2))) },
        &mut out,
    );
    assert!(domain.confirming().expect("confirmation remains").changed, "revision movement marks confirmation");
    step(&mut domain, &env(0), Event::Act { action: Action::Confirm }, &mut out);
    assert!(out.is_empty(), "changed revision never sends");
    assert_eq!(
        domain.confirming().expect("confirmation remains").problem,
        Some(Problem::Changed),
        "problem is visible"
    );
}

#[test]
fn left_before_answer_stays_deciding_then_reports_other_decider() {
    let (mut domain, mut out, _, task_stream, object) = task_started(204);
    step(&mut domain, &env(0), Event::Act { action: Action::Intend { intent: Intent::Release, object } }, &mut out);
    step(&mut domain, &env(0), Event::Act { action: Action::Confirm }, &mut out);
    let Request::Save { .. } = pop(&mut out) else { panic!("decision saved") };
    let Request::Send { request, .. } = pop(&mut out) else { panic!("decision sent") };
    let other = Person { number: 2, name: Box::from(b"Bea".as_slice()) };
    let why = Why::Decided { by: other.clone(), choice: Choice::Rejected, at: Wall::from_nanos(2) };
    step(
        &mut domain,
        &env(2),
        Event::Streamed {
            stream: task_stream,
            event: StreamEvent::Change(Change::Left { key: ObjectKey::Escalation { task: 42 }, why: why.clone() }),
        },
        &mut out,
    );
    let Card::Deciding { .. } = &domain.object(object).expect("card stays").card else {
        panic!("pending answer keeps deciding")
    };
    step(
        &mut domain,
        &env(3),
        Event::Answered {
            request,
            answer: Answer::Done(Outcome::DecidedBefore {
                by: other,
                choice: Choice::Rejected,
                at: Wall::from_nanos(2),
            }),
        },
        &mut out,
    );
    let Request::Save { .. } = pop(&mut out) else { panic!("answer saved") };
    let Card::Leaving { why: shown, .. } = &domain.object(object).expect("card lingers").card else {
        panic!("decision displayed")
    };
    assert_eq!(*shown, why, "other decider's result is shown");
    step(
        &mut domain,
        &env(3),
        Event::Streamed {
            stream: task_stream,
            event: StreamEvent::Change(Change::Left { key: ObjectKey::Escalation { task: 42 }, why: Why::Withdrawn }),
        },
        &mut out,
    );
    let Card::Leaving { why: after, .. } = &domain.object(object).expect("card remains").card else {
        panic!("still leaving")
    };
    assert_eq!(*after, why, "later Left does not overwrite durable answer");
}

#[test]
fn stale_handle_is_refused_after_navigation() {
    let (mut domain, mut out, _, task_stream, object) = task_started(205);
    step(&mut domain, &env(0), Event::Act { action: Action::Go { address: Address::Chats } }, &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream: task_stream }, "old page watch closes");
    let Request::Read { .. } = pop(&mut out) else { panic!("chats read opens") };
    let Request::Address { .. } = pop(&mut out) else { panic!("new address pushed") };
    assert!(domain.object(object).is_none(), "old page object is inaccessible immediately");
    step(&mut domain, &env(0), Event::Act { action: Action::Intend { intent: Intent::Release, object } }, &mut out);
    assert!(domain.confirming().is_none(), "stale card never opens confirmation");
    assert_eq!(
        domain.notices().iter().next().expect("stale notice").kind,
        NoticeKind::StaleObject,
        "stale press is visible"
    );
}

#[test]
fn gone_task_reads_its_result() {
    let (mut domain, mut out, _, task_stream, _) = task_started(206);
    step(&mut domain, &env(0), Event::Ended { stream: task_stream, end: StreamEnd::Gone }, &mut out);
    let Request::Read { read: escalation_read, query: Query::Escalation { task: 42 } } = pop(&mut out) else {
        panic!("gone task reads escalation")
    };
    step(&mut domain, &env(0), Event::Read { read: escalation_read, result: ReadResult::Escalation(None) }, &mut out);
    let Request::Read { read: result_read, query: Query::Result { task: 42 } } = pop(&mut out) else {
        panic!("gone task reads result")
    };
    step(
        &mut domain,
        &env(0),
        Event::Read {
            read: result_read,
            result: ReadResult::Result(Some(TaskResult {
                task: 42,
                kind: EndKind::Done,
                words: Box::from(b"# Fixed login".as_slice()),
                revision: 2,
            })),
        },
        &mut out,
    );
    let Page::Task(page) = domain.page() else { panic!("task page remains") };
    let result = domain.object(page.result.expect("result card exists")).expect("result object exists");
    let Body::Ended(report) = &result.body else { panic!("result has typed body") };
    assert_eq!(report.words.as_ref(), b"# Fixed login", "Markdown words are kept whole");
}

#[test]
fn replacing_task_watch_stays_behind_until_new_snapshot() {
    let (mut domain, mut out, _, old_stream, _) = task_started(207);
    assert_eq!(domain.link(), LinkState::Live, "both original watches are live");
    step(
        &mut domain,
        &env(1),
        Event::Act { action: Action::Go { address: Address::Task { number: 43, section: None } } },
        &mut out,
    );
    assert_eq!(pop(&mut out), Request::Close { stream: old_stream }, "old task watch closes");
    let Request::Address { address: Address::Task { number: 43, .. }, .. } = pop(&mut out) else {
        panic!("new task address is pushed")
    };
    assert_eq!(domain.link(), LinkState::Behind, "a waiting task watch cannot appear live");
    step(&mut domain, &env(2), Event::Ended { stream: old_stream, end: StreamEnd::Closed }, &mut out);
    let Request::Open { stream: new_stream, watch: Watch::Task { number: 43 } } = pop(&mut out) else {
        panic!("new task watch opens after old terminal")
    };
    assert_eq!(domain.link(), LinkState::Behind, "opening is still behind");
    step(&mut domain, &env(3), Event::Opened { stream: new_stream }, &mut out);
    assert_eq!(domain.link(), LinkState::Behind, "Opened is not a snapshot");
    let mut next = snapshot(1);
    next.chip.task = 43;
    next.escalation.as_mut().expect("held task").task = 43;
    step(
        &mut domain,
        &env(4),
        Event::Streamed { stream: new_stream, event: StreamEvent::Snapshot(Snapshot::Task(next)) },
        &mut out,
    );
    assert_eq!(domain.link(), LinkState::Live, "both followed watches have snapshots");
}

#[test]
fn unreachable_gone_read_retries_after_frame_reconnect() {
    let (mut domain, mut out, frame_stream, task_stream, _) = task_started(208);
    step(&mut domain, &env(1), Event::Ended { stream: task_stream, end: StreamEnd::Gone }, &mut out);
    let Request::Read { read, query: Query::Escalation { task: 42 } } = pop(&mut out) else {
        panic!("gone task starts fallback read")
    };
    step(&mut domain, &env(2), Event::Read { read, result: ReadResult::Unreachable }, &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream: frame_stream }, "disconnect closes frame watch");
    match domain.link() {
        LinkState::Offline { .. } => {}
        LinkState::Starting | LinkState::Live | LinkState::Behind => panic!("read failure marks offline"),
    }
    step(&mut domain, &env(3), Event::Ended { stream: frame_stream, end: StreamEnd::Closed }, &mut out);
    let at = domain.next_deadline().expect("reopen timer armed");
    fire(&mut domain, &env(at.as_nanos()), &mut out);
    assert_eq!(pop(&mut out), Request::Open { stream: frame_stream, watch: Watch::Person }, "frame reopens");
    step(&mut domain, &env(at.as_nanos()), Event::Opened { stream: frame_stream }, &mut out);
    step(
        &mut domain,
        &env(at.as_nanos()),
        Event::Streamed { stream: frame_stream, event: StreamEvent::Snapshot(Snapshot::Person(person())) },
        &mut out,
    );
    let Request::Read { query: Query::Escalation { task: 42 }, .. } = pop(&mut out) else {
        panic!("fallback read reissued after frame snapshot")
    };
    assert_eq!(domain.link(), LinkState::Behind, "task page has no live watch after Gone");
}

#[test]
fn navigation_reuses_in_flight_close_and_retires_backoff_watch() {
    let (mut domain, mut out, _, task_stream, _) = task_started(209);
    step(
        &mut domain,
        &env(1),
        Event::Streamed { stream: task_stream, event: StreamEvent::Missed { count: 1 } },
        &mut out,
    );
    assert_eq!(pop(&mut out), Request::Close { stream: task_stream }, "missed asks to close once");
    step(&mut domain, &env(2), Event::Act { action: Action::Go { address: Address::Chats } }, &mut out);
    let Request::Read { .. } = pop(&mut out) else { panic!("chat read opens") };
    let Request::Address { .. } = pop(&mut out) else { panic!("chat address is pushed") };
    assert!(out.is_empty(), "navigation reuses the in flight Close");
    step(&mut domain, &env(3), Event::Ended { stream: task_stream, end: StreamEnd::Closed }, &mut out);
    assert!(out.is_empty(), "old page watch terminal is consumed");

    step(
        &mut domain,
        &env(4),
        Event::Act { action: Action::Go { address: Address::Task { number: 42, section: None } } },
        &mut out,
    );
    let Request::Open { stream: again, watch: Watch::Task { number: 42 } } = pop(&mut out) else {
        panic!("task watch reopens")
    };
    let Request::Address { .. } = pop(&mut out) else { panic!("task address is pushed") };
    step(&mut domain, &env(5), Event::Ended { stream: again, end: StreamEnd::Dropped }, &mut out);
    assert!(out.is_empty(), "dropped terminal only arms retry");
    step(&mut domain, &env(6), Event::Act { action: Action::Go { address: Address::Chats } }, &mut out);
    let Request::Read { .. } = pop(&mut out) else { panic!("chat read opens again") };
    let Request::Address { .. } = pop(&mut out) else { panic!("chat address is pushed again") };
    assert!(out.is_empty(), "retired backoff watch has no live Open to close");
}
