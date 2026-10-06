//! Focused W1 steps: shell ordering, durable keys and watch recovery.
use crate::*;
use alloc::boxed::Box;
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};

fn limits() -> Limits {
    Limits {
        objects: 4,
        requests: 4,
        streams: 1,
        reads: 2,
        window: 4,
        turns: 4,
        tree: 4,
        drafts: 1,
        notices: 4,
        words: 64,
        text: 64,
        streaming: 64,
        projects: 2,
        backoff: Backoff { first: Duration::from_millis(10), most: Duration::from_secs(1) },
        heartbeat: Duration::from_secs(5),
        linger: Duration::from_secs(1),
        notice: Duration::from_secs(2),
        save: Duration::from_millis(50),
        facts: 4,
    }
}

fn env(now: u64) -> Env<Limits> {
    Env { now: Time::from_nanos(now), wall: Wall::from_nanos(now), limits: limits() }
}

fn output() -> Queue<Request> {
    Queue::with_capacity(max_out(&limits()))
}

fn pop(out: &mut Queue<Request>) -> Request {
    out.pop().expect("step emitted an output")
}

fn started(seed: u64) -> (Domain, Queue<Request>, Token, Token) {
    let mut domain = Domain::new(&limits(), seed);
    let mut out = output();
    step(&mut domain, &env(0), Event::Start { address: Address::Chats, saved: None, offset: Offset(3600) }, &mut out);
    let Request::Open { stream, watch: Watch::Person } = pop(&mut out) else { panic!("expected person watch") };
    let Request::Read { read, query: Query::Chats { project: None, .. } } = pop(&mut out) else {
        panic!("expected chats read")
    };
    assert!(out.is_empty(), "start has exactly two outputs");
    (domain, out, stream, read)
}

fn signed_in(domain: &mut Domain, out: &mut Queue<Request>, stream: Token) {
    step(domain, &env(0), Event::Opened { stream }, out);
    step(
        domain,
        &env(0),
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 2,
            })),
        },
        out,
    );
    assert!(out.is_empty(), "first snapshot emits no output");
}

fn submit(domain: &mut Domain, out: &mut Queue<Request>) -> (Token, Key) {
    step(
        domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"Fix login".as_slice()) } },
        out,
    );
    assert!(out.is_empty(), "edit is saved later");
    step(domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, out);
    let Request::Save { saved } = pop(out) else { panic!("expected save") };
    assert_eq!(saved.pending.len(), 1, "key is stored before send");
    assert_eq!(saved.drafts.first().expect("draft exists").text.as_ref(), b"Fix login", "draft is stored");
    let Request::Send { request, key, ask: Ask::StartChat { project: 7, words } } = pop(out) else {
        panic!("expected send")
    };
    assert_eq!(words.as_ref(), b"Fix login", "send carries submitted words");
    assert!(out.is_empty(), "submit has exactly save and send");
    (request, key)
}

#[test]
fn start_snapshot_read_and_durable_chat() {
    let (mut domain, mut out, stream, read) = started(11);
    assert_eq!(domain.link(), LinkState::Starting, "link starts without a snapshot");
    assert_eq!(domain.offset(), Offset(3600), "offset comes from Start");
    signed_in(&mut domain, &mut out, stream);
    assert_eq!(domain.link(), LinkState::Live, "snapshot makes link live");
    assert_eq!(domain.frame().project, Some(7), "first project is selected");
    step(
        &mut domain,
        &env(0),
        Event::Read {
            read,
            result: ReadResult::Chats {
                rows: Box::from([ChatLine {
                    task: 3,
                    project: 7,
                    title: Box::from(b"Earlier".as_slice()),
                    live: false,
                    last_activity: Wall::EPOCH,
                }]),
                older: Some(Cursor(19)),
            },
        },
        &mut out,
    );
    let Page::Chats(chats) = domain.page() else { panic!("chats page remains open") };
    assert_eq!(chats.rows.len(), 1, "read populates the window");
    assert_eq!(chats.older, Some(Cursor(19)), "read retains older cursor");
    let (request, _) = submit(&mut domain, &mut out);
    step(&mut domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, &mut out);
    assert!(out.is_empty(), "double press joins the pending chat");
    step(
        &mut domain,
        &env(0),
        Event::Answered { request, answer: Answer::Done(Outcome::Started { task: 42 }) },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("expected save") };
    assert!(saved.pending.is_empty(), "durable answer removes pending key");
    assert_eq!(
        pop(&mut out),
        Request::Address { address: Address::Task { number: 42, section: None }, push: true },
        "started chat opens its task"
    );
    let Page::Missing { .. } = domain.page() else { panic!("W1 task has a missing page") };
    assert_eq!(domain.field(FieldRef::NewChat).expect("field exists").written, 1, "domain cleared the composer");
}

#[test]
fn busy_retries_same_key_and_reload_restores_it() {
    let (mut domain, mut out, stream, _) = started(22);
    signed_in(&mut domain, &mut out, stream);
    let (request, key) = submit(&mut domain, &mut out);
    step(&mut domain, &env(0), Event::Answered { request, answer: Answer::Busy }, &mut out);
    assert!(out.is_empty(), "busy is not durable");
    let at = domain.next_deadline().expect("retry timer armed");
    fire(&mut domain, &env(at.as_nanos()), &mut out);
    let Request::Send { request: again, key: same, .. } = pop(&mut out) else { panic!("expected retry send") };
    assert_eq!(again, request, "retry uses same token");
    assert_eq!(same, key, "retry uses same key");
    let saved = Saved {
        project: Some(7),
        drafts: Box::from([SavedDraft { field: FieldRef::NewChat, text: Box::from(b"Fix login".as_slice()) }]),
        pending: Box::from([SavedPending {
            key,
            ask: Ask::StartChat { project: 7, words: Box::from(b"Fix login".as_slice()) },
        }]),
    };
    let mut reload = Domain::new(&limits(), 99);
    let mut resumed = output();
    step(
        &mut reload,
        &env(0),
        Event::Start { address: Address::Chats, saved: Some(saved), offset: Offset(0) },
        &mut resumed,
    );
    let Request::Send { key: same, .. } = pop(&mut resumed) else { panic!("expected restored send") };
    assert_eq!(same, key, "reload resends original key");
    assert_eq!(reload.field(FieldRef::NewChat).expect("field exists").written, 1, "restore counts as a domain write");
}

#[test]
fn unreachable_parks_until_watch_reopens() {
    let (mut domain, mut out, stream, _) = started(33);
    signed_in(&mut domain, &mut out, stream);
    let (request, key) = submit(&mut domain, &mut out);
    step(&mut domain, &env(0), Event::Answered { request, answer: Answer::Unreachable }, &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream }, "unreachable closes frame watch");
    assert!(domain.link().offline(), "unreachable makes link offline");
    step(&mut domain, &env(0), Event::Ended { stream, end: StreamEnd::Closed }, &mut out);
    assert!(out.is_empty(), "closed watch waits for backoff");
    let at = domain.next_deadline().expect("reopen timer armed");
    fire(&mut domain, &env(at.as_nanos()), &mut out);
    assert_eq!(pop(&mut out), Request::Open { stream, watch: Watch::Person }, "watch reopens");
    step(&mut domain, &env(at.as_nanos()), Event::Opened { stream }, &mut out);
    step(
        &mut domain,
        &env(at.as_nanos()),
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 2,
            })),
        },
        &mut out,
    );
    let Request::Send { key: same, .. } = pop(&mut out) else { panic!("expected resumed send") };
    assert_eq!(same, key, "parked ask resumes with same key");
    assert_eq!(domain.link(), LinkState::Live, "fresh snapshot restores live link");
}

#[test]
fn navigating_abandons_a_read_and_went_does_not_push() {
    let (mut domain, mut out, _, read) = started(44);
    step(&mut domain, &env(0), Event::Went { address: Address::Task { number: 5, section: None } }, &mut out);
    assert!(out.is_empty(), "browser history navigation does not push address");
    step(
        &mut domain,
        &env(0),
        Event::Read { read, result: ReadResult::Chats { rows: Box::from([]), older: None } },
        &mut out,
    );
    let Page::Missing { .. } = domain.page() else { panic!("late read cannot reopen old page") };
}

#[test]
fn signed_out_parks_request_and_shows_sign_in() {
    let (mut domain, mut out, stream, _) = started(55);
    signed_in(&mut domain, &mut out, stream);
    let (request, _) = submit(&mut domain, &mut out);
    step(&mut domain, &env(0), Event::Answered { request, answer: Answer::SignedOut }, &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream }, "sign-out closes frame watch");
    let Page::SignIn { then } = domain.page() else { panic!("sign-in page shown") };
    assert_eq!(*then, Address::Chats, "sign-in returns to chats");
    step(&mut domain, &env(0), Event::Act { action: Action::SignIn }, &mut out);
    assert_eq!(pop(&mut out), Request::SignIn { then: Address::Chats }, "button invokes engine sign-in");
}

#[test]
fn draft_save_is_coalesced_without_writing_back() {
    let (mut domain, mut out, _, _) = started(66);
    step(
        &mut domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"a".as_slice()) } },
        &mut out,
    );
    step(
        &mut domain,
        &env(10),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"ab".as_slice()) } },
        &mut out,
    );
    assert_eq!(
        domain.field(FieldRef::NewChat).expect("field exists").written,
        0,
        "person's edits do not count as domain writes"
    );
    let at = domain.next_deadline().expect("save timer armed");
    fire(&mut domain, &env(at.as_nanos()), &mut out);
    let Request::Save { saved } = pop(&mut out) else { panic!("expected draft save") };
    assert_eq!(saved.drafts.first().expect("draft exists").text.as_ref(), b"ab", "latest draft saved");
}

#[test]
fn limits_reject_invalid_configuration() {
    let mut invalid = limits();
    invalid.streams = 0;
    assert!(worst_case(&invalid).is_none(), "frame needs one stream");
    invalid = limits();
    invalid.backoff.most = Duration::from_millis(1);
    assert!(worst_case(&invalid).is_none(), "maximum backoff is at least first");
    assert!(worst_case(&limits()).is_some(), "normal limits validate");
}

#[test]
fn missed_reopens_immediately_and_silence_uses_backoff() {
    let (mut domain, mut out, stream, _) = started(77);
    signed_in(&mut domain, &mut out, stream);
    step(&mut domain, &env(0), Event::Streamed { stream, event: StreamEvent::Missed { count: 3 } }, &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream }, "missed delivery closes old watch");
    assert_eq!(domain.link(), LinkState::Behind, "missed delivery shows behind");
    step(&mut domain, &env(0), Event::Ended { stream, end: StreamEnd::Closed }, &mut out);
    assert_eq!(pop(&mut out), Request::Open { stream, watch: Watch::Person }, "missed watch reopens at once");
    step(&mut domain, &env(0), Event::Opened { stream }, &mut out);
    step(
        &mut domain,
        &env(0),
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 2,
            })),
        },
        &mut out,
    );
    assert_eq!(domain.link(), LinkState::Live, "new snapshot clears behind");
    let silent = domain.next_deadline().expect("heartbeat armed");
    fire(&mut domain, &env(silent.as_nanos()), &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream }, "silent watch closes");
    assert!(domain.link().offline(), "silence means offline");
    step(&mut domain, &env(silent.as_nanos()), Event::Ended { stream, end: StreamEnd::Closed }, &mut out);
    assert!(out.is_empty(), "silent watch waits for backoff");
    let reopen = domain.next_deadline().expect("reopen armed");
    assert!(reopen > silent, "backoff follows silence");
    fire(&mut domain, &env(reopen.as_nanos()), &mut out);
    assert_eq!(pop(&mut out), Request::Open { stream, watch: Watch::Person }, "watch reopens after backoff");
}

#[test]
fn distinct_chats_fill_request_bound_and_preserve_newer_draft() {
    let (mut domain, mut out, stream, _) = started(88);
    signed_in(&mut domain, &mut out, stream);
    let (first, first_key) = submit(&mut domain, &mut out);
    for words in [b"two".as_slice(), b"three".as_slice(), b"four".as_slice()] {
        step(
            &mut domain,
            &env(0),
            Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(words) } },
            &mut out,
        );
        step(&mut domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, &mut out);
        let Request::Save { saved } = pop(&mut out) else { panic!("each new chat is saved") };
        assert!(saved.pending.len() <= 4, "saved pending count fits limit");
        let Request::Send { key, .. } = pop(&mut out) else { panic!("each new chat is sent") };
        assert_ne!(key, first_key, "distinct intents use fresh keys");
    }
    step(
        &mut domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"five".as_slice()) } },
        &mut out,
    );
    step(&mut domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, &mut out);
    assert!(out.is_empty(), "full slab emits no untracked send");
    assert_eq!(
        domain.notices().iter().next().expect("full notice exists").kind,
        NoticeKind::RequestFull,
        "full request bound is visible"
    );
    step(
        &mut domain,
        &env(0),
        Event::Answered { request: first, answer: Answer::Done(Outcome::Started { task: 41 }) },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("durable answer saves") };
    assert_eq!(saved.pending.len(), 3, "only answered key retired");
    assert_eq!(
        domain.field(FieldRef::NewChat).expect("field exists").text.as_ref(),
        b"five",
        "answer preserves newly typed draft"
    );
}

#[test]
fn read_capacity_refuses_new_page_without_a_stuck_spinner() {
    let (mut domain, mut out, _, _) = started(99);
    for address in [
        Address::Task { number: 1, section: None },
        Address::Chats,
        Address::Task { number: 2, section: None },
        Address::Chats,
    ] {
        step(&mut domain, &env(0), Event::Went { address }, &mut out);
    }
    let Request::Read { .. } = pop(&mut out) else { panic!("second chats page reads") };
    assert!(out.is_empty(), "third chats page cannot read with two slots held");
    let Page::Chats(chats) = domain.page() else { panic!("chats page remains") };
    assert!(!chats.loading, "read refusal clears loading state");
    assert_eq!(
        domain.notices().iter().next().expect("read notice exists").kind,
        NoticeKind::ReadFull,
        "read limit is visible"
    );
}

#[test]
fn dropped_watch_recovers_and_resends_a_parked_request() {
    let (mut domain, mut out, stream, _) = started(111);
    signed_in(&mut domain, &mut out, stream);
    let (request, key) = submit(&mut domain, &mut out);
    step(&mut domain, &env(0), Event::Ended { stream, end: StreamEnd::Dropped }, &mut out);
    assert!(domain.link().offline(), "dropped watch makes link offline");
    assert!(out.is_empty(), "dropped watch waits for backoff");
    step(&mut domain, &env(0), Event::Answered { request, answer: Answer::Unreachable }, &mut out);
    assert!(out.is_empty(), "no close follows a completed dropped watch");
    let at = domain.next_deadline().expect("reopen timer exists");
    fire(&mut domain, &env(at.as_nanos()), &mut out);
    assert_eq!(pop(&mut out), Request::Open { stream, watch: Watch::Person }, "watch reopens");
    step(&mut domain, &env(at.as_nanos()), Event::Opened { stream }, &mut out);
    step(
        &mut domain,
        &env(at.as_nanos()),
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 2,
            })),
        },
        &mut out,
    );
    let Request::Send { key: same, .. } = pop(&mut out) else { panic!("parked request resumes") };
    assert_eq!(same, key, "reconnect preserves key");
}

#[test]
fn signed_out_watch_waits_for_in_flight_terminal() {
    let (mut domain, mut out, stream, _) = started(122);
    signed_in(&mut domain, &mut out, stream);
    let (request, _) = submit(&mut domain, &mut out);
    step(&mut domain, &env(0), Event::Ended { stream, end: StreamEnd::SignedOut }, &mut out);
    assert!(out.is_empty(), "ended watch is not closed a second time");
    let Page::SignIn { .. } = domain.page() else { panic!("signed-out watch shows sign-in") };
    step(&mut domain, &env(0), Event::Answered { request, answer: Answer::SignedOut }, &mut out);
    assert!(out.is_empty(), "in-flight send still receives one terminal");
}
