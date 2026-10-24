use super::fixtures::{self, chat as f};
use crate::pages::{
    Act, Request,
    chat::{Chat, Sending},
};
use jig_client::{Event, Readable, Told, Watchable, contract as c};
use skein_lib::Queue;
const OUTPUTS: u32 = 16;
const SERIAL: u64 = 37;
const EMPTY: u32 = 0;
const REPLY: u64 = 1;
#[test]
fn opening_a_chat_watches_task_and_reads_its_first_activity_page() {
    let mut out = Queue::with_capacity(OUTPUTS);
    let chat = Chat::new(f::TASK, f::FIRST, &fixtures::limits(), &mut out);
    assert_eq!(
        out.pop(),
        Some(Request::Client(Event::Watch {
            handle: chat.document,
            subject: Watchable::Jig(c::Subject::Task { task: f::TASK })
        }))
    );
    assert_eq!(
        out.pop(),
        Some(Request::Client(Event::Read {
            handle: chat.activity,
            read: Readable::Jig(c::Read::Activity { task: f::TASK, before: None })
        }))
    );
    assert!(out.is_empty());
}
#[test]
fn a_live_run_is_watched_and_dropped_when_it_ends() {
    let mut f = f::opened();
    let run = f.chat.run.expect("live run watched");
    assert_eq!(
        f.out.pop(),
        Some(Request::Client(Event::Watch {
            handle: run,
            subject: Watchable::Jig(c::Subject::Run { task: f::TASK, turn: None, offset: EMPTY })
        }))
    );
    let mut task = f::task();
    f::phase(&mut task, c::OwnerChipPhase::Active(c::RunStage::Idle));
    f::deliver(&mut f.client, f.watch, task);
    f.chat.told(&f.client, Told::Changed { handle: f.chat.document }, &mut f.out);
    assert_eq!(f.chat.run, None);
    assert_eq!(f::handles(&mut f.out).as_ref(), [run]);
}
#[test]
fn leaving_a_chat_unwatches_its_task_activity_and_run() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    f.chat.leave(&mut f.out);
    assert_eq!(f::handles(&mut f.out).as_ref(), [f.chat.document, f.chat.activity, f.chat.run.expect("run")]);
}
fn open_run(f: &mut f::Fixture) -> skein_lib::Token {
    let mut emitted = Queue::with_capacity(OUTPUTS);
    while let Some(Request::Client(event)) = f.out.pop() {
        f.client.step(skein_lib::Time::ZERO, skein_lib::Wall::EPOCH, event, &mut emitted);
    }
    while let Some(request) = emitted.pop() {
        if let jig_client::Request::Down {
            down: jig_client::Down::Open { watch, subject: Watchable::Jig(c::Subject::Run { .. }) },
        } = request
        {
            return watch;
        }
    }
    panic!("run opened")
}
fn run_text(from: u32, bytes: &[u8]) -> c::RunText {
    let length = u32::try_from(bytes.len()).unwrap();
    c::RunText {
        attempt: f::ATTEMPT,
        began: Some(skein_lib::Wall::EPOCH),
        turn: c::ProvisionalTurnName { attempt: f::ATTEMPT, number: u32::try_from(f::POSITION).unwrap(), retry: 0 },
        from,
        through: from.checked_add(length).unwrap(),
        text: c::Text::new(Box::from(bytes), length).unwrap(),
        cut: false,
        call: None,
    }
}
fn run_delivery(f: &mut f::Fixture, watch: skein_lib::Token, delivery: c::Delivery) {
    f.client.step(
        skein_lib::Time::ZERO,
        skein_lib::Wall::EPOCH,
        Event::Up { up: jig_client::Up::Watched { watch, event: c::WatchEvent::Delivered(delivery) } },
        &mut Queue::with_capacity(OUTPUTS),
    );
    let handle = f.chat.run.expect("run watched");
    for told in [Told::Changed { handle }, Told::Text { handle }] {
        assert!(f.chat.told(&f.client, told, &mut f.out));
    }
}
fn run_state(writing: Option<c::RunText>) -> c::Delivery {
    c::Delivery::RunState(Box::new(c::RunState {
        task: f::TASK,
        attempt: Some(f::ATTEMPT),
        committed_turn: 0,
        writing,
        call: None,
    }))
}
#[test]
fn a_chats_stream_suffix_extends_its_provisional_turn_until_committed() {
    let mut f = f::opened();
    let watch = open_run(&mut f);
    run_delivery(&mut f, watch, run_state(Some(run_text(0, b"The answer"))));
    assert_eq!(f.chat.thread.live.get(0).expect("provisional prefix").text.bytes(), b"The answer");
    run_delivery(&mut f, watch, c::Delivery::Run(Box::new(run_text(10, b" is forty-two."))));
    let handle = f.chat.run.unwrap();
    assert_eq!(f.client.text(handle).unwrap().text.bytes(), b"The answer is forty-two.");
    assert_eq!(f.chat.thread.live.get(0).expect("whole provisional turn").text.bytes(), b"The answer is forty-two.");

    let mut blocks = skein_lib::List::with_capacity(1);
    blocks.push(c::Block::Text(fixtures::acts::text(b"The answer is forty-two."))).unwrap();
    f::activity_blocks(&mut f, c::Speaker::Assistant, blocks, f::POSITION);
    assert!(f.chat.thread.live.is_empty(), "the committed turn replaces the provisional turn");
    let lines = f.chat.thread.lines();
    let [jig_client::thread::Line::Turn { turn, .. }] = lines.as_slice() else { panic!("one committed turn") };
    assert_eq!(
        turn.messages.get(0).unwrap().blocks.as_slice(),
        [c::Block::Text(fixtures::acts::text(b"The answer is forty-two."))]
    );
}
#[test]
fn a_chats_first_stream_text_follows_a_state_without_writing() {
    let mut f = f::opened();
    let watch = open_run(&mut f);
    run_delivery(&mut f, watch, run_state(None));
    assert!(f.chat.thread.live.is_empty());
    run_delivery(&mut f, watch, c::Delivery::Run(Box::new(run_text(0, b"The answer"))));
    assert_eq!(f.chat.thread.live.get(0).expect("first provisional turn").text.bytes(), b"The answer");
}
#[test]
fn a_chats_stream_can_arrive_before_its_first_run_state() {
    let mut f = f::opened();
    let watch = open_run(&mut f);
    run_delivery(&mut f, watch, c::Delivery::Run(Box::new(run_text(0, b"The answer"))));
    assert_eq!(f.chat.thread.attempt, Some(f::ATTEMPT));
    assert_eq!(f.chat.thread.live.get(0).expect("first provisional turn").text.bytes(), b"The answer");
}
#[test]
fn stop_sends_one_stop_and_shows_it_pending_until_answered() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    let mut serial = SERIAL;
    for _ in [false, true] {
        f.chat.act(&fixtures::env(&fixtures::limits()), &f.client, Act::Stop, &mut serial, &mut f.out);
    }
    assert_eq!(fixtures::acts::sent(&mut f.out).as_ref(), [c::Ask::Stop { project: fixtures::PROJECT, task: f::TASK }]);
    assert_eq!(
        f.chat.pending.as_slice(),
        [(jig_client::Key { asker: crate::owner::Owner::Pages.asker(), number: SERIAL + 1 }, Sending::Stop)]
    );
    let key = f.chat.pending.as_slice()[0].0;
    f.chat.told(&f.client, Told::Answered { key, outcome: jig_client::Outcome::Unknown }, &mut f.out);
    assert!(f.chat.pending.is_empty());
    assert_eq!(f.chat.outcome, Some(jig_client::Outcome::Unknown));
}
fn chat_pages() -> (crate::pages::Pages, jig_client::Client) {
    let mut pages = crate::pages::Pages::new();
    let client = fixtures::acts::live_client();
    crate::pages::step(
        &mut pages,
        &fixtures::env(&fixtures::limits()),
        &client,
        crate::pages::Event::Went { address: crate::Address::Chat { task: f::TASK } },
        &mut Queue::with_capacity(OUTPUTS),
    );
    (pages, client)
}
fn chat_told(pages: &mut crate::pages::Pages, client: &jig_client::Client, told: Told) -> Queue<Request> {
    let mut out = Queue::with_capacity(OUTPUTS);
    crate::pages::step(
        pages,
        &fixtures::env(&fixtures::limits()),
        client,
        crate::pages::Event::Told { told },
        &mut out,
    );
    out
}
fn owner_activity(position: u64, line: c::OwnerActivity) -> c::ActivityLine {
    c::ActivityLine {
        position,
        at: skein_lib::Wall::EPOCH,
        event: c::Activity::Owner {
            source: Box::new(c::OwnerActivityLine { task: f::TASK, position, at: skein_lib::Wall::EPOCH, line }),
            decoded: None,
        },
    }
}
fn owner_turn(position: u64, reply: u64) -> c::ActivityLine {
    owner_activity(
        position,
        c::OwnerActivity::Turn { reply, attempt: f::ATTEMPT, turn: u32::try_from(reply).unwrap(), read: None },
    )
}
fn chat_move(
    pages: &mut crate::pages::Pages,
    client: &jig_client::Client,
    position: u64,
    reply: u64,
) -> Queue<Request> {
    let mut rows = skein_lib::List::with_capacity(1);
    rows.push(owner_turn(position, reply)).unwrap();
    chat_activity(pages, client, position, rows)
}
fn chat_activity(
    pages: &mut crate::pages::Pages,
    client: &jig_client::Client,
    position: u64,
    rows: skein_lib::List<c::ActivityLine>,
) -> Queue<Request> {
    let mut task = f::task();
    task.activity = position;
    let document = pages.chat().unwrap().document;
    chat_told(
        pages,
        client,
        Told::Page {
            handle: document,
            page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Task(Box::new(task)))),
        },
    );
    chat_told(
        pages,
        client,
        Told::Page {
            handle: pages.chat().unwrap().activity,
            page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Activity(Box::new(c::ActivityPage {
                rows,
                before: None,
                after: None,
                cut: false,
            })))),
        },
    )
}
fn seen_key(pages: &crate::pages::Pages) -> jig_client::Key {
    for (key, kind) in &pages.chat().unwrap().pending {
        if *kind == Sending::Seen {
            return *key;
        }
    }
    panic!("automatic Seen pending")
}
fn seen_answer() -> jig_client::Outcome {
    jig_client::Outcome::Answered(jig_client::Answered::Jig(c::Answer::Done(c::Outcome::Changed)))
}
fn seen_ask(through: u64) -> c::Ask {
    c::Ask::Seen { through: c::SeenThrough::Conversation { task: f::TASK, through } }
}
#[test]
fn a_chats_first_loaded_page_is_seen_automatically() {
    let (mut pages, client) = chat_pages();
    assert_eq!(
        fixtures::acts::sent(&mut chat_move(&mut pages, &client, f::POSITION, REPLY)).as_ref(),
        [seen_ask(REPLY)]
    );
}
#[test]
fn turns_arriving_while_a_chat_is_open_are_seen_through_the_newest() {
    let (mut pages, client) = chat_pages();
    chat_move(&mut pages, &client, f::POSITION, REPLY);
    let key = seen_key(&pages);
    chat_told(&mut pages, &client, Told::Answered { key, outcome: seen_answer() });
    assert_eq!(
        fixtures::acts::sent(&mut chat_move(&mut pages, &client, f::POSITION + 2, REPLY + 1)).as_ref(),
        [seen_ask(REPLY + 1)]
    );
}
#[test]
fn a_chat_waits_for_the_pending_seen_before_sending_the_newest_reply() {
    let (mut pages, client) = chat_pages();
    chat_move(&mut pages, &client, f::POSITION, REPLY);
    let key = seen_key(&pages);
    assert!(fixtures::acts::sent(&mut chat_move(&mut pages, &client, f::POSITION + 1, REPLY + 1)).is_empty());
    let mut out = chat_told(&mut pages, &client, Told::Answered { key, outcome: seen_answer() });
    assert_eq!(fixtures::acts::sent(&mut out).as_ref(), [seen_ask(REPLY + 1)]);
    assert_ne!(seen_key(&pages), key);
}
#[test]
fn a_refused_or_failed_chat_seen_is_resent_at_the_next_move() {
    for outcome in [
        jig_client::Outcome::Unknown,
        jig_client::Outcome::Unsent,
        jig_client::Outcome::Lost,
        jig_client::Outcome::Answered(jig_client::Answered::Jig(c::Answer::Refused {
            refusal: c::Refusal::NotOffered,
            at: None,
        })),
    ] {
        let (mut pages, client) = chat_pages();
        chat_move(&mut pages, &client, f::POSITION, REPLY);
        let key = seen_key(&pages);
        assert!(fixtures::acts::sent(&mut chat_told(&mut pages, &client, Told::Answered { key, outcome })).is_empty());
        assert_eq!(pages.chat().unwrap().seen, 0);
        // A fresh page delivery is a move even when it repeats the current tail.
        let handle = pages.chat().unwrap().document;
        let mut out = chat_told(
            &mut pages,
            &client,
            Told::Page {
                handle,
                page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Task(Box::new(f::task())))),
            },
        );
        assert_eq!(fixtures::acts::sent(&mut out).as_ref(), [seen_ask(REPLY)]);
        assert_ne!(seen_key(&pages), key);
    }
}
#[test]
fn a_chat_sends_no_seen_without_a_new_loaded_reply() {
    let (mut pages, client) = chat_pages();
    chat_move(&mut pages, &client, f::POSITION, REPLY);
    let key = seen_key(&pages);
    chat_told(&mut pages, &client, Told::Answered { key, outcome: seen_answer() });
    assert!(fixtures::acts::sent(&mut chat_move(&mut pages, &client, f::POSITION, REPLY)).is_empty());
    assert!(fixtures::acts::sent(&mut chat_move(&mut pages, &client, f::POSITION - 1, REPLY)).is_empty());
    assert_eq!(pages.chat().unwrap().seen, REPLY);
}
#[test]
fn a_chat_sees_reply_numbers_instead_of_activity_positions() {
    let (mut pages, client) = chat_pages();
    let mut rows = skein_lib::List::with_capacity(4);
    rows.push(owner_activity(
        1,
        c::OwnerActivity::RunBegan { attempt: f::ATTEMPT, why: c::OwnerRunWhy::Made, resumed: false },
    ))
    .unwrap();
    rows.push(owner_activity(
        2,
        c::OwnerActivity::Words {
            message: 1,
            by: c::OwnerRequester::Person(fixtures::PARTY),
            kind: c::OwnerMessageKind::Words,
            words: fixtures::acts::text(b"Hello"),
        },
    ))
    .unwrap();
    rows.push(owner_turn(3, 1)).unwrap();
    rows.push(owner_activity(
        4,
        c::OwnerActivity::RunEnded { attempt: f::ATTEMPT, end: c::OwnerRunEnd::Parked, spent: 0 },
    ))
    .unwrap();
    let mut out = chat_activity(&mut pages, &client, 4, rows);
    assert_eq!(pages.chat().unwrap().thread.loaded, 4);
    assert_eq!(fixtures::acts::sent(&mut out).as_ref(), [seen_ask(1)]);
    assert_eq!(pages.chat().unwrap().seen, 1);
    let key = seen_key(&pages);
    chat_told(&mut pages, &client, Told::Answered { key, outcome: seen_answer() });
    assert_eq!(fixtures::acts::sent(&mut chat_move(&mut pages, &client, 5, 2)).as_ref(), [seen_ask(2)]);
    assert_eq!(pages.chat().unwrap().seen, 2);
}
#[test]
fn a_chat_with_activity_but_no_numbered_owner_turn_sends_no_seen() {
    let (mut pages, client) = chat_pages();
    let mut rows = skein_lib::List::with_capacity(3);
    rows.push(owner_activity(
        1,
        c::OwnerActivity::RunBegan { attempt: f::ATTEMPT, why: c::OwnerRunWhy::Made, resumed: false },
    ))
    .unwrap();
    rows.push(owner_activity(
        2,
        c::OwnerActivity::Words {
            message: 1,
            by: c::OwnerRequester::Person(fixtures::PARTY),
            kind: c::OwnerMessageKind::Words,
            words: fixtures::acts::text(b"Hello"),
        },
    ))
    .unwrap();
    rows.push(c::ActivityLine {
        position: 4,
        at: skein_lib::Wall::EPOCH,
        event: c::Activity::Turn {
            name: c::TurnName { attempt: f::ATTEMPT, number: 1 },
            turn: c::TurnRead::Undecodable { why: c::TurnProblem::Malformed },
        },
    })
    .unwrap();
    assert!(fixtures::acts::sent(&mut chat_activity(&mut pages, &client, 4, rows)).is_empty());
    assert_eq!(pages.chat().unwrap().thread.loaded, 4);
    assert_eq!(pages.chat().unwrap().seen, 0);
    assert!(pages.chat().unwrap().pending.is_empty());
}
#[test]
fn automatic_seen_preserves_the_last_acts_outcome_on_send_and_answer() {
    let mut f = f::opened();
    let mut serial = SERIAL;
    f.chat.act(&fixtures::env(&fixtures::limits()), &f.client, Act::Stop, &mut serial, &mut f.out);
    let key = f.chat.pending.as_slice()[0].0;
    f.chat.told(&f.client, Told::Answered { key, outcome: jig_client::Outcome::Unknown }, &mut f.out);
    let mut rows = skein_lib::List::with_capacity(1);
    rows.push(owner_turn(f::POSITION, REPLY)).unwrap();
    f.chat.told(
        &f.client,
        Told::Page {
            handle: f.chat.activity,
            page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Activity(Box::new(c::ActivityPage {
                rows,
                before: None,
                after: None,
                cut: false,
            })))),
        },
        &mut f.out,
    );
    f.chat.see(&mut serial, &mut f.out);
    assert_eq!(f.chat.outcome, Some(jig_client::Outcome::Unknown));
    assert_eq!(f.chat.outcome_kind, Some(Sending::Stop));
    let key = f.chat.pending.as_slice()[0].0;
    f.chat.told(&f.client, Told::Answered { key, outcome: seen_answer() }, &mut f.out);
    assert_eq!(f.chat.outcome, Some(jig_client::Outcome::Unknown));
    assert_eq!(f.chat.outcome_kind, Some(Sending::Stop));
}
fn root_event(root: &mut crate::Domain, event: crate::Event, out: &mut Queue<crate::Request>) {
    crate::step(root, &fixtures::env(&fixtures::limits()), event, out);
}
fn chat_with_loaded_turn() -> (crate::Domain, Queue<crate::Request>) {
    let limits = fixtures::limits();
    let mut root = crate::unlinked(&limits, fixtures::SEED);
    let me = fixtures::start(&mut root, skein_lib::List::with_capacity(0));
    let mut out = Queue::with_capacity(crate::max_out(&limits));
    fixtures::confirm(&mut root, me, fixtures::PARTY, &mut out);
    root_event(
        &mut root,
        crate::Event::Went { place: crate::Place::Address(crate::Address::Chat { task: f::TASK }) },
        &mut out,
    );
    let mut watch = None;
    while let Some(request) = out.pop() {
        if let crate::Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Open { watch: token, subject: Watchable::Jig(c::Subject::Task { .. }) },
                },
        } = request
        {
            watch = Some(token);
        }
    }
    let mut task = f::task();
    task.acts = fixtures::acts::offered(c::ActKind::Message, c::Admission::Allowed);
    root_event(
        &mut root,
        crate::Event::Client {
            event: Event::Up {
                up: jig_client::Up::Watched {
                    watch: watch.expect("chat task watch"),
                    event: c::WatchEvent::Delivered(c::Delivery::Task(Box::new(task))),
                },
            },
        },
        &mut out,
    );
    let mut rows = skein_lib::List::with_capacity(1);
    rows.push(owner_turn(f::POSITION, REPLY)).unwrap();
    let handle = root.pages().chat().unwrap().activity;
    crate::route::told(
        &mut root,
        &fixtures::env(&limits),
        Told::Page {
            handle,
            page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Activity(Box::new(c::ActivityPage {
                rows,
                before: None,
                after: None,
                cut: false,
            })))),
        },
        &mut out,
    );
    (root, out)
}
#[test]
fn an_automatic_seen_and_a_message_can_both_be_pending_and_answered() {
    const REQUESTS: u32 = 2;
    let (mut root, mut out) = chat_with_loaded_turn();
    for act in [Act::MessageWords { words: Box::from(*b"Hello") }, Act::Send] {
        root_event(&mut root, crate::Event::Act { action: crate::Action::Pages(act) }, &mut out);
    }
    let pending = root.pages().chat().unwrap().pending.clone();
    let mut kinds = skein_lib::List::with_capacity(REQUESTS);
    for (_, kind) in &pending {
        kinds.push(*kind).unwrap();
    }
    assert_eq!(kinds.as_slice(), [Sending::Seen, Sending::Message]);
    for (key, _) in &pending {
        assert!(root.client().pending(*key).is_some(), "both requests admitted to the real table");
    }
    root_event(&mut root, crate::Event::Client { event: Event::Stored }, &mut out);
    let mut attempts = skein_lib::List::with_capacity(REQUESTS);
    let mut asks = skein_lib::List::with_capacity(REQUESTS);
    while let Some(request) = out.pop() {
        if let crate::Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Send { attempt, request: jig_client::Ask::Jig(ask), .. },
                },
        } = request
        {
            attempts.push(attempt).unwrap();
            asks.push(*ask).unwrap();
        }
    }
    assert_eq!(
        asks.as_slice(),
        [
            seen_ask(REPLY),
            c::Ask::Message { project: fixtures::PROJECT, task: f::TASK, words: fixtures::acts::text(b"Hello") },
        ]
    );
    for (attempt, (_, kind)) in attempts.iter().zip(&pending) {
        let answer = match kind {
            Sending::Seen => c::Answer::Done(c::Outcome::Changed),
            Sending::Message => c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::Said { task: f::TASK, message: 1 })),
            Sending::Stop | Sending::Close => panic!("only Seen and Message pending"),
        };
        root_event(
            &mut root,
            crate::Event::Client {
                event: Event::Up {
                    up: jig_client::Up::Answer {
                        attempt: *attempt,
                        envelope: skein_lib::RequestEnvelope::Final,
                        answer: Some(jig_client::Answered::Jig(answer)),
                        why: None,
                    },
                },
            },
            &mut out,
        );
    }
    assert!(root.pages().chat().unwrap().pending.is_empty());
    for (key, _) in &pending {
        assert!(root.client().pending(*key).is_none(), "both requests answered");
    }
    assert_eq!(root.pages().chat().unwrap().seen, REPLY);
    assert_eq!(root.pages().chat().unwrap().outcome_kind, Some(Sending::Message));
}

#[test]
fn closing_a_chat_with_a_live_delegate_is_one_compound_request() {
    let mut f = f::with_delegate();
    f.chat.prepare_close(&f.client);
    assert!(fixtures::acts::sent(&mut f.out).is_empty(), "said before sent");
    f.chat.delegate_fate(f::DELEGATE, jig_client::shapes::Fate::Move);
    let mut serial = SERIAL;
    f.chat.close(&mut serial, &mut f.out);
    f.chat.close(&mut serial, &mut f.out);
    let expected = jig_client::shapes::close_chat(
        &fixtures::chats::me(),
        fixtures::PROJECT,
        f::TASK,
        &[jig_client::shapes::Delegate { task: f::DELEGATE, fate: jig_client::shapes::Fate::Move }],
        fixtures::acts::text(b"Chat closed"),
    )
    .expect("chat shape");
    assert_eq!(fixtures::acts::sent(&mut f.out).as_ref(), [expected], "jig's compound, once");
}
#[test]
fn the_confirmed_close_keeps_the_delegate_fates_and_grace_shown() {
    let mut f = f::with_delegate();
    f.chat.prepare_close(&f.client);
    let shown = f.chat.closing.clone().expect("confirmation request");
    let mut changed = f::task();
    changed.acts = fixtures::acts::offered(c::ActKind::Cancel, c::Admission::Allowed);
    f::deliver(&mut f.client, f.watch, changed);
    let mut serial = SERIAL;
    f.chat.close(&mut serial, &mut f.out);
    assert_eq!(fixtures::acts::sent(&mut f.out).as_ref(), [shown], "confirm the shown request");
}

const FIRST_CARD: u32 = 0;
const SECOND_CARD: u32 = 1;
const PROPOSAL: c::ProposalKey = fixtures::acts::PROPOSAL;
const DELEGATE_PROPOSAL: c::ProposalKey = c::ProposalKey { proposer: c::Proposer::Task(f::DELEGATE), number: 4 };
const PROPOSALS: u32 = 2;
fn proposals(f: &mut f::Fixture, keys: &[c::ProposalKey]) {
    let mut task = f::task();
    task.proposals = skein_lib::List::with_capacity(PROPOSALS);
    for key in keys {
        task.proposals.push(*key).expect("proposals fit");
    }
    f::deliver(&mut f.client, f.watch, task);
    f.chat.told(&f.client, Told::Changed { handle: f.chat.document }, &mut f.out);
}
fn reads(out: &mut Queue<Request>) -> Box<[(jig_client::Handle, c::ProposalKey)]> {
    let mut reads = skein_lib::List::with_capacity(PROPOSALS);
    while let Some(request) = out.pop() {
        if let Request::Client(Event::Read { handle, read: Readable::Jig(c::Read::Proposal { proposal }) }) = request {
            reads.push((handle, proposal)).expect("bounded reads");
        }
    }
    reads.into_boxed()
}
#[test]
fn a_chats_proposals_are_read_once_and_kept_in_the_tasks_order() {
    let mut f = f::opened();
    proposals(&mut f, &[PROPOSAL, DELEGATE_PROPOSAL]);
    let first = reads(&mut f.out);
    assert_eq!(first.len(), 2);
    assert_eq!([first[0].1, first[1].1], [PROPOSAL, DELEGATE_PROPOSAL]);
    proposals(&mut f, &[DELEGATE_PROPOSAL, PROPOSAL]);
    assert!(reads(&mut f.out).is_empty(), "a retained proposal is not read again");
    assert_eq!(
        [f.chat.cards.get(FIRST_CARD).expect("first").handle, f.chat.cards.get(SECOND_CARD).expect("second").handle],
        [first[1].0, first[0].0],
        "delegate's proposal kept, ordered by the task"
    );
}
#[test]
fn a_proposal_decided_elsewhere_leaves_the_chats_card() {
    let mut f = f::opened();
    proposals(&mut f, &[PROPOSAL]);
    let handle = reads(&mut f.out)[0].0;
    proposals(&mut f, &[]);
    assert!(f.chat.cards.is_empty());
    assert_eq!(f::handles(&mut f.out).as_ref(), [handle], "drop the card's handle too");
}
fn rejecting_pages() -> crate::pages::Pages {
    let mut pages = crate::pages::Pages::new();
    crate::pages::step(
        &mut pages,
        &fixtures::env(&fixtures::limits()),
        crate::unlinked(&fixtures::limits(), fixtures::SEED).client(),
        crate::pages::Event::Went { address: crate::Address::Chat { task: f::TASK } },
        &mut Queue::with_capacity(OUTPUTS),
    );
    let handle = jig_client::Handle { asker: crate::owner::Owner::Pages.asker(), number: f::FIRST };
    pages
        .cards_mut()
        .expect("chat cards")
        .push(crate::pages::inbox::Card {
            object: c::ObjectKey::Proposal(PROPOSAL),
            group: c::InboxGroup::Proposals,
            project: Some(fixtures::PROJECT),
            position: None,
            unread: None,
            handle,
            page: Some(fixtures::acts::proposal(c::ActKind::Reject, c::Admission::Allowed)),
            words: None,
            key: None,
            outcome: None,
        })
        .expect("card fits");
    pages
}
#[test]
fn rejecting_a_proposal_in_place_is_said_before_it_is_sent() {
    let mut pages = rejecting_pages();
    let object = c::ObjectKey::Proposal(PROPOSAL);
    fixtures::acts::act(&mut pages, Act::CardWords { object, words: Box::from(*b"Too costly") });
    let mut said = fixtures::acts::act(&mut pages, fixtures::acts::decide(object, c::ActKind::Reject));
    assert!(fixtures::acts::sent(&mut said).is_empty());
    assert_eq!(pages.confirmation().expect("confirmation").act, c::ActKind::Reject);
    assert_eq!(pages.chat().expect("chat").cards.get(FIRST_CARD).expect("card").key, None);
    assert_eq!(
        fixtures::acts::sent(&mut fixtures::acts::act(&mut pages, Act::Confirm)).as_ref(),
        [c::Ask::Decide {
            project: fixtures::PROJECT,
            on: c::DecideOn::Proposal {
                proposal: PROPOSAL,
                running: None,
                decision: c::Decision::Rejected { reason: fixtures::acts::text(b"Too costly") }
            }
        }]
    );
}

#[test]
fn a_departed_proposal_keeps_its_identity_for_the_explanation() {
    let mut f = f::opened();
    proposals(&mut f, &[PROPOSAL]);
    proposals(&mut f, &[]);
    assert_eq!(f.chat.departures(), &[c::ObjectKey::Proposal(PROPOSAL)]);
}

const DELEGATED: u64 = 2;
fn live_delegate() -> c::Task {
    let mut task = f::task();
    task.delegates = skein_lib::List::with_capacity(1);
    let mut chip = task.chip.clone();
    chip.task = DELEGATED;
    task.delegates.push(c::LiveDelegate { chip, takeover_budget: 0 }).expect("live delegate");
    task
}
#[test]
fn a_live_delegate_is_watched_before_its_call_names_it() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    let handle = jig_client::Handle { asker: crate::owner::Owner::Pages.asker(), number: f.chat.next };
    f::deliver(&mut f.client, f.watch, live_delegate());
    f.chat.told(&f.client, Told::Changed { handle: f.chat.document }, &mut f.out);
    assert_eq!(watches(&mut f.out).as_ref(), [(handle, DELEGATED)]);
}
#[test]
fn a_delegate_that_ends_is_unwatched_once_nothing_names_it() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    f::deliver(&mut f.client, f.watch, live_delegate());
    f.chat.told(&f.client, Told::Changed { handle: f.chat.document }, &mut f.out);
    let handle = watches(&mut f.out)[0].0;
    let mut task = f::task();
    task.delegates = skein_lib::List::with_capacity(0);
    f::deliver(&mut f.client, f.watch, task);
    f.chat.told(&f.client, Told::Changed { handle: f.chat.document }, &mut f.out);
    assert_eq!(f::handles(&mut f.out).as_ref(), [handle]);
}
#[test]
fn a_chat_watches_a_delegated_task_when_only_its_result_remains() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    let next = f.chat.next;
    let mut blocks = skein_lib::List::with_capacity(1);
    blocks
        .push(c::Block::Result {
            call: 8,
            summary: c::Text::new(Box::from(f::WAKE_SUMMARY), 128).expect("wake summary"),
            failed: false,
        })
        .expect("wake result");
    f::activity_blocks(&mut f, c::Speaker::Person, blocks, f::POSITION);
    let handle = jig_client::Handle { asker: crate::owner::Owner::Pages.asker(), number: next };
    assert_eq!(watches(&mut f.out).as_ref(), [(handle, DELEGATED)]);
}

fn watches(out: &mut Queue<Request>) -> Box<[(jig_client::Handle, u64)]> {
    let mut watches = skein_lib::List::with_capacity(OUTPUTS);
    while let Some(request) = out.pop() {
        if let Request::Client(Event::Watch { handle, subject: Watchable::Jig(c::Subject::Task { task }) }) = request {
            watches.push((handle, task)).expect("bounded watches");
        }
    }
    watches.into_boxed()
}
#[test]
fn a_chat_watches_each_delegated_task_as_its_call_arrives() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    let next = f.chat.next;
    f::activity(&mut f, &[DELEGATED], f::POSITION);
    let handle = jig_client::Handle { asker: crate::owner::Owner::Pages.asker(), number: next };
    assert_eq!(watches(&mut f.out).as_ref(), [(handle, DELEGATED)], "fresh handle watches the delegated task");
    assert!(f.chat.delegation(&f.client, DELEGATED).is_none(), "wait for its document");
    let mut emitted = Queue::with_capacity(OUTPUTS);
    f.client.step(
        skein_lib::Time::ZERO,
        skein_lib::Wall::EPOCH,
        Event::Watch { handle, subject: Watchable::Jig(c::Subject::Task { task: DELEGATED }) },
        &mut emitted,
    );
    let mut watch = None;
    while let Some(request) = emitted.pop() {
        if let jig_client::Request::Down {
            down: jig_client::Down::Open { watch: token, subject: Watchable::Jig(c::Subject::Task { task: DELEGATED }) },
        } = request
        {
            watch = Some(token);
        }
    }
    let mut task = f::change();
    task.chip.task = DELEGATED;
    f::deliver(&mut f.client, watch.expect("delegation opened"), task.clone());
    assert!(f.chat.told(&f.client, Told::Changed { handle }, &mut f.out));
    assert_eq!(f.chat.delegation(&f.client, DELEGATED), Some(&task));
    assert!(f.out.is_empty(), "a document refresh leaves its watch alone");
    assert!(f.chat.delegation(&f.client, 999).is_none(), "only held delegations are exposed");
}
#[test]
fn a_delegation_requested_by_another_owner_is_not_this_chats() {
    let mut f = f::opened();
    f::activity(&mut f, &[DELEGATED], f::POSITION);
    for (requester, requested) in
        [(c::OwnerRequester::Person(DELEGATED), false), (c::OwnerRequester::Task(f::TASK), true)]
    {
        let mut task = f::change();
        task.chip.task = DELEGATED;
        task.requester = requester;
        f.chat.delegations.get_mut(0).expect("delegation").final_task = Some(Box::new(task.clone()));
        assert_eq!(f.chat.delegation(&f.client, DELEGATED), Some(&task));
        assert_eq!(f.chat.requested(&f.client, DELEGATED), if requested { Some(&task) } else { None });
    }
}
#[test]
fn a_chat_watches_the_newest_delegations_up_to_its_bound() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    let bound = crate::pages::delegation::bound(&fixtures::limits());
    let mut tasks = skein_lib::List::with_capacity(bound + 1);
    for task in 1..=u64::from(bound + 1) {
        tasks.push(task).expect("calls");
    }
    f::activity(&mut f, tasks.as_slice(), f::POSITION);
    let watched = watches(&mut f.out);
    assert_eq!(watched.len(), usize::try_from(bound).expect("bound fits"));
    for (index, (_, task)) in watched.iter().enumerate() {
        assert_eq!(*task, u64::from(bound + 1) - u64::try_from(index).expect("bounded index"));
    }
    for delegation in &f.chat.delegations {
        assert_ne!(delegation.task, 1, "oldest is past the bound");
    }
}
#[test]
fn a_delegation_is_unwatched_as_its_call_leaves_the_thread() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    f::activity(&mut f, &[DELEGATED], f::POSITION);
    let handle = watches(&mut f.out)[0].0;
    f::activity(&mut f, &[], f::POSITION + 1);
    assert_eq!(f::handles(&mut f.out).as_ref(), [handle]);
    assert!(f.chat.delegations.is_empty());
    assert!(f.chat.delegation(&f.client, DELEGATED).is_none());
}
#[test]
fn a_newer_delegation_unwatches_the_one_past_the_bound() {
    let mut f = f::opened();
    while f.out.pop().is_some() {}
    let bound = crate::pages::delegation::bound(&fixtures::limits());
    let mut tasks = skein_lib::List::with_capacity(bound + 1);
    for task in 1..=u64::from(bound) {
        tasks.push(task).expect("calls");
    }
    f::activity(&mut f, tasks.as_slice(), f::POSITION);
    let old = watches(&mut f.out);
    let oldest = old.last().expect("oldest").0;
    tasks.push(u64::from(bound + 1)).expect("new call");
    f::activity(&mut f, tasks.as_slice(), f::POSITION + 1);
    assert_eq!(
        f.out.pop(),
        Some(Request::Client(Event::Read {
            handle: f.chat.activity,
            read: Readable::Jig(c::Read::Activity { task: f::TASK, before: None })
        }))
    );
    assert_eq!(f.out.pop(), Some(Request::Client(Event::Unwatch { handle: oldest })), "make watch room first");
    let new = watches(&mut f.out);
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].1, u64::from(bound + 1));
    for (delegation, (handle, task)) in f.chat.delegations.iter().skip(1).zip(old.iter()) {
        assert_eq!((delegation.handle, delegation.task), (*handle, *task), "retained handles stay the same");
    }
}
#[test]
fn leaving_a_chat_unwatches_its_delegations() {
    let mut f = f::with_delegate();
    f::activity(&mut f, &[DELEGATED, 3], f::POSITION);
    let watched = watches(&mut f.out);
    f.chat.leave(&mut f.out);
    let left = f::handles(&mut f.out);
    assert_eq!(left.len(), 5, "document, activity, run and both delegations");
    assert_eq!(&left[3..], [watched[0].0, watched[1].0]);
}
#[test]
fn an_ended_delegation_reads_its_task_and_keeps_its_result() {
    let mut f = f::opened();
    f::activity(&mut f, &[DELEGATED], f::POSITION);
    let handle = watches(&mut f.out)[0].0;
    for why in [jig_client::Why::SubjectEnded, jig_client::Why::Unknown] {
        assert!(f.chat.told(&f.client, Told::Ended { handle, why }, &mut f.out));
        assert_eq!(
            f.out.pop(),
            Some(Request::Client(Event::Read { handle, read: Readable::Jig(c::Read::Task { task: DELEGATED }) }))
        );
        assert!(f.out.is_empty());
    }
    let mut task = f::change();
    task.chip.task = DELEGATED;
    let result = f::result(&task, c::Ending::Done, Some(f::change_value(b"abcdef0123456789")));
    let historical = c::OwnerHistorical {
        key: c::ObjectKey::Task(DELEGATED),
        value: c::OwnerHistoricalValue::Task { document: Box::new(task.clone()), result: Box::new(result.clone()) },
        at: None,
        by: None,
    };
    assert!(f.chat.told(
        &f.client,
        Told::Page {
            handle,
            page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Historical(Box::new(historical))))
        },
        &mut f.out
    ));
    task.result = Some(c::TaskResult::Document(Box::new(result)));
    assert_eq!(f.chat.delegation(&f.client, DELEGATED), Some(&task));
    f.chat.told(&f.client, Told::Changed { handle: f.chat.document }, &mut f.out);
    assert_eq!(f.chat.delegation(&f.client, DELEGATED), Some(&task), "refresh keeps the final task");
    assert!(f.out.is_empty(), "no new requests for retained calls");
}

#[test]
fn the_chat_composer_keeps_the_send_bound_when_receiving_grows() {
    let mut limits = fixtures::limits();
    limits.client.domain.text = 2048;
    limits.client.domain.turn_text = 2048;
    let mut f = f::opened();
    let mut serial = SERIAL;
    let bound = usize::try_from(temper_client_protocol_wire::CUE_BYTES).unwrap();
    let mut words = skein_lib::bytes::zeroed(bound);
    words.fill(b'a');
    let mut over = skein_lib::bytes::zeroed(bound.checked_add(1).unwrap());
    over.fill(b'b');
    f.chat.act(&fixtures::env(&limits), &f.client, Act::MessageWords { words: words.clone() }, &mut serial, &mut f.out);
    assert_eq!(f.chat.words, words);
    f.chat.act(&fixtures::env(&limits), &f.client, Act::MessageWords { words: over }, &mut serial, &mut f.out);
    assert_eq!(f.chat.words, words, "an oversized edit keeps the previous words");
}

fn window_page(chat: &mut Chat, client: &jig_client::Client, out: &mut Queue<Request>, page: u32, pages: u32) {
    let mut rows = skein_lib::List::with_capacity(2);
    for offset in 0..2 {
        rows.push(c::ActivityLine {
            position: u64::from(pages.checked_sub(page).unwrap().checked_mul(2).unwrap().checked_sub(offset).unwrap()),
            at: skein_lib::Wall::EPOCH,
            event: c::Activity::RunBegan {
                attempt: 1,
                how: c::RunBeginning::Resumed,
                why: fixtures::acts::text(b"resume"),
            },
        })
        .unwrap();
    }
    let cursor = c::Cursor::new(Box::from(*b"older"), 5).unwrap();
    chat.told(
        client,
        Told::Page {
            handle: chat.activity,
            page: jig_client::Readout::Jig(c::ReadResult::Value(c::ReadValue::Activity(Box::new(c::ActivityPage {
                rows,
                before: if page.checked_add(1).unwrap() < pages { Some(cursor) } else { None },
                after: None,
                cut: false,
            })))),
        },
        out,
    );
}
#[test]
fn a_chat_fills_its_window_to_the_start_without_offering_older() {
    let limits = fixtures::limits();
    let client = jig_client::Client::unlinked(&limits.client, fixtures::SEED).unwrap();
    let mut out = Queue::with_capacity(OUTPUTS);
    let mut chat = Chat::new(f::TASK, f::FIRST, &limits, &mut out);
    while out.pop().is_some() {}
    for page in 0..5 {
        window_page(&mut chat, &client, &mut out, page, 5);
        assert_eq!(!out.is_empty(), page < 4, "page {page}");
        while out.pop().is_some() {}
        assert!(!chat.offers_older());
    }
    assert_eq!(chat.thread.lines.rows.len(), 10);
    assert!(chat.thread.reading.is_none());
}
#[test]
fn a_full_chat_window_offers_older_and_reads_it_when_asked() {
    let limits = fixtures::limits();
    let client = jig_client::Client::unlinked(&limits.client, fixtures::SEED).unwrap();
    let mut out = Queue::with_capacity(OUTPUTS);
    let mut chat = Chat::new(f::TASK, f::FIRST, &limits, &mut out);
    while out.pop().is_some() {}
    for page in 0..8 {
        window_page(&mut chat, &client, &mut out, page, 10);
        assert_eq!(!out.is_empty(), page < 7, "page {page}");
        while out.pop().is_some() {}
    }
    assert_eq!(chat.thread.lines.rows.len(), crate::pages::chat::LINES);
    assert!(chat.offers_older());
    let mut serial = SERIAL;
    chat.act(&fixtures::env(&limits), &client, Act::Older, &mut serial, &mut out);
    let Some(Request::Client(Event::Read { read: Readable::Jig(c::Read::Activity { before: Some(_), .. }), .. })) =
        out.pop()
    else {
        panic!("Older reads the preceding page");
    };
    window_page(&mut chat, &client, &mut out, 8, 10);
    assert!(out.is_empty());
    assert!(chat.offers_older());
}

#[test]
fn a_change_call_names_its_task_in_the_chats_delegations() {
    let mut call = f::delegate_call(DELEGATED);
    call.tool = fixtures::acts::text(b"change");
    let mut delegated = skein_lib::List::with_capacity(2);
    crate::pages::delegation::delegated(&call, &mut delegated);
    assert_eq!(delegated.as_slice(), &[DELEGATED]);
}

#[test]
fn a_change_call_without_objects_reads_the_delegated_numbers() {
    let mut call = f::delegate_call(DELEGATED);
    call.tool = fixtures::acts::text(b"change");
    call.objects = skein_lib::List::with_capacity(0);
    let mut delegated = skein_lib::List::with_capacity(2);
    crate::pages::delegation::delegated(&call, &mut delegated);
    assert_eq!(delegated.as_slice(), &[2, 3]);
}
