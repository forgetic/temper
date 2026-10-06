//! Focused W1 steps: shell ordering, durable keys and watch recovery.
use crate::*;
use alloc::boxed::Box;
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};

pub(super) fn limits() -> Limits {
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
    let Request::Save { saved } = pop(out) else { panic!("edit saves immediately") };
    assert_eq!(
        saved.drafts.first().expect("new chat draft exists").text.as_ref(),
        b"Fix login",
        "edit is durable before reload"
    );
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
    let Page::Task(task) = domain.page() else { panic!("started chat opens task page") };
    assert_eq!(task.number, 42, "task page names started chat");
    assert_eq!(domain.field(FieldRef::NewChat).expect("field exists").written, 1, "domain cleared the composer");
}

#[test]
fn later_same_words_edit_survives_older_start_answer() {
    let (mut domain, mut out, stream, _) = started(12);
    signed_in(&mut domain, &mut out, stream);
    let (request, _) = submit(&mut domain, &mut out);
    step(
        &mut domain,
        &env(1),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"Fix login".as_slice()) } },
        &mut out,
    );
    let Request::Save { .. } = pop(&mut out) else { panic!("later edit is saved") };
    step(
        &mut domain,
        &env(2),
        Event::Answered { request, answer: Answer::Done(Outcome::Started { task: 42 }) },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("answer saves state") };
    assert_eq!(saved.drafts[0].text.as_ref(), b"Fix login", "later unsent edit survives old answer");
    assert_eq!(domain.field(FieldRef::NewChat).expect("composer").written, 0, "domain does not clear later edit");
}

#[test]
fn restored_start_answer_clears_only_its_submitted_draft() {
    for later_same_words in [false, true] {
        let (mut domain, mut out, stream, _) = started(13);
        signed_in(&mut domain, &mut out, stream);
        step(
            &mut domain,
            &env(0),
            Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"Fix login".as_slice()) } },
            &mut out,
        );
        let Request::Save { .. } = pop(&mut out) else { panic!("draft edit saved") };
        step(&mut domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, &mut out);
        let Request::Save { mut saved } = pop(&mut out) else { panic!("submitted ask saved") };
        let Request::Send { key, .. } = pop(&mut out) else { panic!("submitted ask sent") };
        if later_same_words {
            step(
                &mut domain,
                &env(1),
                Event::Act {
                    action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"Fix login".as_slice()) },
                },
                &mut out,
            );
            let Request::Save { saved: later } = pop(&mut out) else { panic!("later edit saved") };
            saved = later;
        }
        let mut restored = Domain::new(&limits(), 14);
        let mut resumed = output();
        step(
            &mut restored,
            &env(2),
            Event::Start { address: Address::Chats, saved: Some(saved), offset: Offset(0) },
            &mut resumed,
        );
        let Request::Open { stream, watch: Watch::Person } = pop(&mut resumed) else {
            panic!("identity watch opens before replay")
        };
        assert!(resumed.is_empty(), "saved ask waits for its person's snapshot");
        step(&mut restored, &env(2), Event::Opened { stream }, &mut resumed);
        step(
            &mut restored,
            &env(2),
            Event::Streamed {
                stream,
                event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                    person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                    projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                    inbox_count: 2,
                })),
            },
            &mut resumed,
        );
        let mut replay = None;
        let mut replay_key = None;
        while let Some(request) = resumed.pop() {
            if let Request::Send { request, key, .. } = request {
                replay = Some(request);
                replay_key = Some(key);
            }
        }
        let replay = replay.expect("matching person replays the saved ask");
        assert_eq!(replay_key, Some(key), "replay retains the key");
        step(
            &mut restored,
            &env(3),
            Event::Answered { request: replay, answer: Answer::Done(Outcome::Started { task: 42 }) },
            &mut resumed,
        );
        let Request::Save { saved: answered } = pop(&mut resumed) else { panic!("answer updates storage") };
        let expected = if later_same_words { b"Fix login".as_slice() } else { b"".as_slice() };
        assert_eq!(answered.drafts[0].text.as_ref(), expected, "later unsent edit survives, submitted draft clears");
        assert_eq!(restored.field(FieldRef::NewChat).expect("composer").text.as_ref(), expected);
    }
}

#[test]
fn saved_request_waits_for_its_person_across_account_switch() {
    let (mut domain, mut out, stream, _) = started(16);
    signed_in(&mut domain, &mut out, stream);
    step(
        &mut domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"A's words".as_slice()) } },
        &mut out,
    );
    let Request::Save { .. } = pop(&mut out) else { panic!("draft saved") };
    step(&mut domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, &mut out);
    let Request::Save { saved } = pop(&mut out) else { panic!("ask saved") };
    let Request::Send { key, .. } = pop(&mut out) else { panic!("ask sent for A") };
    assert_eq!(saved.person, Some(1), "saved ask is bound to A");

    let mut restored = Domain::new(&limits(), 17);
    let mut resumed = output();
    step(
        &mut restored,
        &env(1),
        Event::Start { address: Address::Chats, saved: Some(saved), offset: Offset(0) },
        &mut resumed,
    );
    let Request::Open { stream, watch: Watch::Person } = pop(&mut resumed) else { panic!("person watch opens") };
    assert!(resumed.is_empty(), "no replay before identity is known");
    let Page::Starting = restored.page() else { panic!("A's draft is hidden before identity is known") };
    step(&mut restored, &env(1), Event::Opened { stream }, &mut resumed);
    step(
        &mut restored,
        &env(1),
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                person: Person { number: 2, name: Box::from(b"Bea".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 0,
            })),
        },
        &mut resumed,
    );
    assert!(restored.account_mismatch(), "B cannot take over A's saved ask");
    let Page::SignIn { .. } = restored.page() else { panic!("A's draft remains hidden from B") };
    assert!(resumed.is_empty(), "B's snapshot sends no request");
    step(
        &mut restored,
        &env(2),
        Event::Streamed {
            stream,
            event: StreamEvent::Change(Change::Person(PersonSnapshot {
                person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 0,
            })),
        },
        &mut resumed,
    );
    let mut replayed = false;
    for request in &resumed {
        if let Request::Send { key: same, .. } = request {
            assert_eq!(*same, key, "A resumes the same keyed ask");
            replayed = true;
        }
    }
    assert!(replayed, "A's return releases the parked ask");
    assert!(!restored.account_mismatch(), "the account warning clears for A");
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
        person: Some(1),
        project: Some(7),
        drafts: Box::from([SavedDraft {
            field: FieldRef::NewChat,
            text: Box::from(b"Fix login".as_slice()),
            edit_version: 1,
            target: None,
        }]),
        pending: Box::from([SavedPending {
            key,
            ask: Ask::StartChat { project: 7, words: Box::from(b"Fix login".as_slice()) },
            draft_version: Some(1),
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
    let Request::Open { stream, watch: Watch::Person } = pop(&mut resumed) else { panic!("person watch opens") };
    assert!(resumed.is_empty(), "reload waits for the person snapshot");
    step(&mut reload, &env(0), Event::Opened { stream }, &mut resumed);
    step(
        &mut reload,
        &env(0),
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Person(PersonSnapshot {
                person: Person { number: 1, name: Box::from(b"Ada".as_slice()) },
                projects: Box::from([Project { number: 7, name: Box::from(b"Temper".as_slice()) }]),
                inbox_count: 2,
            })),
        },
        &mut resumed,
    );
    let mut replayed = false;
    for request in &resumed {
        if let Request::Send { key: same, .. } = request {
            assert_eq!(*same, key, "reload resends original key under the same person");
            replayed = true;
        }
    }
    assert!(replayed, "matching person snapshot releases saved ask");
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
fn unreachable_chats_read_retries_after_watch_recovery() {
    let (mut domain, mut out, stream, read) = started(34);
    signed_in(&mut domain, &mut out, stream);
    step(&mut domain, &env(0), Event::Read { read, result: ReadResult::Unreachable }, &mut out);
    assert_eq!(pop(&mut out), Request::Close { stream }, "unreachable read closes the watch");
    let Page::Chats(chats) = domain.page() else { panic!("chats page remains open") };
    assert!(chats.loading, "page waits for the retry");
    step(&mut domain, &env(0), Event::Ended { stream, end: StreamEnd::Closed }, &mut out);
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
    let Request::Read { read: again, query: Query::Chats { .. } } = pop(&mut out) else {
        panic!("chats read retries after the snapshot")
    };
    step(
        &mut domain,
        &env(at.as_nanos()),
        Event::Read { read: again, result: ReadResult::Chats { rows: Box::from([]), older: None } },
        &mut out,
    );
    let Page::Chats(chats) = domain.page() else { panic!("chats page remains open") };
    assert!(!chats.loading, "successful retry ends loading");
    assert_eq!(domain.link(), LinkState::Live, "recovered watch makes the link live");
}

#[test]
fn refused_chats_read_ends_loading() {
    let (mut domain, mut out, _, read) = started(35);
    step(&mut domain, &env(0), Event::Read { read, result: ReadResult::Refused(Refusal::Role) }, &mut out);
    let Page::Chats(chats) = domain.page() else { panic!("chats page remains open") };
    assert!(!chats.loading, "a terminal refusal ends loading");
    assert!(out.is_empty(), "a refused read has no shell request");
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
    let Page::Task(task) = domain.page() else { panic!("late read cannot reopen old page") };
    assert_eq!(task.number, 5, "late read leaves task page current");
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
fn draft_edit_saves_each_value_without_writing_back() {
    let (mut domain, mut out, _, _) = started(66);
    step(
        &mut domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"a".as_slice()) } },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("first edit saves") };
    assert_eq!(saved.drafts.first().expect("draft exists").text.as_ref(), b"a", "first value is saved");
    step(
        &mut domain,
        &env(10),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"ab".as_slice()) } },
        &mut out,
    );
    let Request::Save { saved } = pop(&mut out) else { panic!("second edit saves") };
    assert_eq!(saved.drafts.first().expect("draft exists").text.as_ref(), b"ab", "latest draft saved");
    assert_eq!(
        domain.field(FieldRef::NewChat).expect("field exists").written,
        0,
        "person's edits do not count as domain writes"
    );
    assert!(out.is_empty(), "each edit emitted exactly one save");
}

#[test]
fn overlimit_paste_restores_accepted_composer_before_submit() {
    let (mut domain, mut out, stream, _) = started(67);
    signed_in(&mut domain, &mut out, stream);
    step(
        &mut domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"hi".as_slice()) } },
        &mut out,
    );
    let Request::Save { .. } = pop(&mut out) else { panic!("accepted words saved") };
    step(
        &mut domain,
        &env(0),
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from([b'x'; 65]) } },
        &mut out,
    );
    let field = domain.field(FieldRef::NewChat).expect("composer exists");
    assert_eq!(field.text.as_ref(), b"hi", "oversized paste is rejected");
    assert_eq!(field.written, 1, "view must restore the accepted value");
    step(&mut domain, &env(0), Event::Act { action: Action::Submit { form: Form::NewChat } }, &mut out);
    let Request::Save { .. } = pop(&mut out) else { panic!("request saved") };
    let Request::Send { ask: Ask::StartChat { words, .. }, .. } = pop(&mut out) else { panic!("request sent") };
    assert_eq!(words.as_ref(), b"hi", "visible restored words match the submitted ask");
}

#[test]
fn limits_reject_invalid_configuration() {
    let mut invalid = limits();
    invalid.streams = 0;
    assert!(worst_case(&invalid).is_none(), "frame needs one stream");
    invalid = limits();
    invalid.backoff.most = Duration::from_millis(1);
    assert!(worst_case(&invalid).is_none(), "maximum backoff is at least first");
    invalid = limits();
    invalid.words = invalid.text.checked_add(1).expect("test bound fits");
    assert!(worst_case(&invalid).is_none(), "a submitted opening message must fit its task snapshot");
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
        let Request::Save { .. } = pop(&mut out) else { panic!("edit saves") };
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
    let Request::Save { .. } = pop(&mut out) else { panic!("edit saves") };
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
    let mut saw_read_full = false;
    for notice in domain.notices() {
        if notice.kind == NoticeKind::ReadFull {
            saw_read_full = true;
        }
    }
    assert!(saw_read_full, "read limit is visible");
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
