use skein_lib::{Duration, Time, Token};
use temper_channel::{codec, wire};
use temper_engine_domain::{self as engine, forge, notes, views};
use temper_engine_protocol::{
    names, payload,
    translate::{self, Error, Repository, Value},
};
use temper_engine_protocol_world::{SIZES, charter, outcome};

const ITEM: engine::Item = engine::Item { repository: 1, number: 42 };
const CHANNEL: Token = Token::new(9);

#[test]
fn checked_names_are_total_and_peer_pairs_must_agree() {
    let max = engine::Item { repository: 255, number: (1 << 24) - 1 };
    let count = (1 << 32) - 1;
    let run = names::run(max).expect("largest item fits");
    let attempt = names::attempt(max, count).expect("largest attempt fits");
    assert_eq!(attempt.raw(), u64::MAX);
    assert_eq!(names::item(run), Some(max));
    assert_eq!(names::pair(run, attempt), Some((max, count)));
    assert_eq!(names::run(engine::Item { repository: 256, ..max }), None);
    assert_eq!(names::run(engine::Item { number: 1 << 24, ..max }), None);
    assert_eq!(names::attempt(max, 1 << 32), None);
    assert_eq!(names::item(Token::new(run.raw() | (1 << 24))), None);
    assert_eq!(names::pair(names::run(ITEM).expect("fits"), attempt), None);
}

fn assignment() -> engine::Assignment {
    engine::Assignment {
        item: ITEM,
        attempt: 3,
        workspace: engine::Workspace {
            key: b"workspace".as_slice().into(),
            repositories: Box::new([engine::Checkout {
                repository: 1,
                start: engine::Start::Saved { branch: b"saved".as_slice().into() },
                push: Some(b"temper/42".as_slice().into()),
            }]),
        },
        save: Some(b"saved".as_slice().into()),
        charter: charter(),
        snapshot: None,
        grants: Box::new([
            engine::accounts::Grant { account: 7, generation: 3, valid: Duration::from_secs(100) },
            engine::accounts::Grant { account: 9, generation: 0, valid: Duration::from_nanos(u64::MAX) },
        ]),
    }
}
fn repositories() -> Box<[Repository]> {
    Box::new([
        Repository { name: b"zero".as_slice().into(), remote: b"acme/zero".as_slice().into(), identity: 8 },
        Repository { name: b"one".as_slice().into(), remote: b"acme/one".as_slice().into(), identity: 9 },
    ])
}
fn values() -> Box<[Value]> {
    Box::new([
        Value {
            account: 7,
            generation: 3,
            expires: Time::from_nanos(100_000_000_000),
            token: b"access".as_slice().into(),
            account_id: b"provider-id".as_slice().into(),
        },
        Value {
            account: 9,
            generation: 0,
            expires: Time::from_nanos(u64::MAX),
            token: b"git-secret".as_slice().into(),
            account_id: Box::new([]),
        },
    ])
}

#[test]
fn assignments_attach_values_echo_repository_tags_and_clip_relative_validity() {
    let request = engine::Request::Assign { channel: CHANNEL, assignment: assignment() };
    let (_, message) = translate::down(request, &repositories(), &values(), Time::from_nanos(20_000_000_000), &SIZES)
        .expect("fits")
        .expect("link request");
    let bytes = codec::encode(&message, &SIZES).expect("a complete frame");
    assert_eq!(codec::decode(&bytes, &SIZES), Some(message.clone()));
    let wire::Message::Assign { run, attempt, workspace, grants, charter: encoded, .. } = message else {
        panic!("an assignment")
    };
    assert_eq!(names::pair(Token::new(run), Token::new(attempt)), Some((ITEM, 3)));
    assert_eq!(workspace.repositories[0].tag, 1);
    assert_eq!(workspace.repositories[0].identity, 9);
    assert_eq!(grants[0].valid, Duration::from_secs(80));
    assert_eq!(&*grants[0].token, b"access");
    assert_eq!(payload::decode_charter(&encoded, &SIZES), Some(charter()));
    let request = engine::Request::Assign { channel: CHANNEL, assignment: assignment() };
    assert_eq!(translate::down(request, &repositories(), &[], Time::ZERO, &SIZES), Err(Error::Credential));
    let old = engine::Request::Grant {
        channel: CHANNEL,
        item: ITEM,
        attempt: 3,
        grant: engine::accounts::Grant { account: 7, generation: 2, valid: Duration::from_secs(10) },
    };
    assert_eq!(translate::down(old, &repositories(), &values(), Time::ZERO, &SIZES), Ok(None));
}

#[test]
fn complete_opaque_snapshots_keep_unknown_agent_versions_across_park_and_assign() {
    let snapshot: Box<[u8]> = [0, 99, 4, 5, 6].into();
    let message = wire::Message::Answer {
        run: names::run(ITEM).expect("fits").raw(),
        attempt: names::attempt(ITEM, 3).expect("fits").raw(),
        answer: wire::LinkAnswer::Parked {
            snapshot: Some(snapshot.clone()),
            work: wire::Work { landed: Box::new([]), saved: None },
        },
    };
    let event = translate::up(CHANNEL, message, 2, &SIZES).expect("the engine does not read an agent snapshot");
    let Some(engine::Event::Answer { answer: engine::Answer::Parked { snapshot: Some(kept), .. }, .. }) = event else {
        panic!("parked snapshot kept")
    };
    assert_eq!(kept, snapshot);
    let mut assigned = assignment();
    assigned.snapshot = Some(kept);
    let (_, message) = translate::down(
        engine::Request::Assign { channel: CHANNEL, assignment: assigned },
        &repositories(),
        &values(),
        Time::ZERO,
        &SIZES,
    )
    .expect("fits")
    .expect("an assignment");
    let wire::Message::Assign { snapshot: Some(sent), .. } = message else {
        panic!("the stored snapshot is reassigned")
    };
    assert_eq!(sent, snapshot);
}

#[test]
fn malformed_names_payloads_repository_tags_and_directions_make_no_event() {
    let run = names::run(ITEM).expect("fits").raw();
    let attempt = names::attempt(engine::Item { number: 43, ..ITEM }, 3).expect("fits").raw();
    let mismatch = wire::Message::Bounced { run, attempt, event: 1, bounce: wire::Bounce::Full };
    assert_eq!(translate::up(CHANNEL, mismatch, 2, &SIZES), Err(Error::Name));
    let invalid = wire::Message::Told {
        run,
        attempt: names::attempt(ITEM, 3).expect("fits").raw(),
        fact: b"raw fact".as_slice().into(),
    };
    assert_eq!(translate::up(CHANNEL, invalid, 2, &SIZES), Ok(None));
    let invalid = wire::Message::Hello {
        slots: 1,
        workstreams: Box::new([]),
        hosting: Box::new([wire::Hosting {
            run: names::run(engine::Item { repository: 2, number: 1 }).expect("fits").raw(),
            attempt: names::attempt(engine::Item { repository: 2, number: 1 }, 1).expect("fits").raw(),
            phase: wire::HostingPhase::Active,
        }]),
    };
    assert_eq!(translate::up(CHANNEL, invalid, 2, &SIZES), Err(Error::Repository));
    assert_eq!(translate::up(CHANNEL, wire::Message::AgentCancel, 2, &SIZES), Err(Error::Direction));
    let terminal = wire::Message::Answer {
        run,
        attempt: names::attempt(ITEM, 3).expect("fits").raw(),
        answer: wire::LinkAnswer::Failed {
            failure: wire::Failure::Run { failure: wire::RunFailure::Exhausted },
            detail: Box::new([]),
            work: wire::Work { landed: Box::new([wire::Landed { tag: 2, commit: [3; 32] }]), saved: None },
        },
    };
    assert_eq!(translate::up(CHANNEL, terminal, 2, &SIZES), Err(Error::Repository));
}

#[test]
fn declared_outcomes_and_exhausted_failures_reach_the_engine_with_tagged_work() {
    let encoded = payload::encode_outcome(&outcome(), &SIZES).expect("fits");
    let run = names::run(ITEM).expect("fits").raw();
    let attempt = names::attempt(ITEM, 3).expect("fits").raw();
    let work = || wire::Work { landed: Box::new([wire::Landed { tag: 1, commit: [4; 32] }]), saved: None };
    let event = translate::up(
        CHANNEL,
        wire::Message::Answer { run, attempt, answer: wire::LinkAnswer::Ended { outcome: encoded, work: work() } },
        2,
        &SIZES,
    )
    .expect("valid");
    let expected = engine::Work { landed: Box::new([engine::Landed { repository: 1, commit: [4; 32] }]) };
    assert_eq!(
        event,
        Some(engine::Event::Answer {
            channel: CHANNEL,
            item: ITEM,
            attempt: 3,
            answer: engine::Answer::Ended { outcome: outcome(), work: expected }
        })
    );
    let event = translate::up(
        CHANNEL,
        wire::Message::Answer {
            run,
            attempt,
            answer: wire::LinkAnswer::Failed {
                failure: wire::Failure::Run { failure: wire::RunFailure::Exhausted },
                detail: b"quota".as_slice().into(),
                work: work(),
            },
        },
        2,
        &SIZES,
    )
    .expect("valid");
    let Some(engine::Event::Answer {
        answer: engine::Answer::Failed { failure: engine::Failure::Transient, .. }, ..
    }) = event
    else {
        panic!("quota exhaustion is transient")
    };
}

fn served() -> engine::Served {
    engine::Served::Recalled {
        entries: Box::new([notes::Entry {
            scope: notes::Scope::Goal { repository: 1, number: 42 },
            name: b"result".as_slice().into(),
            revision: 4,
            page: notes::Page {
                description: b"what happened".as_slice().into(),
                author: notes::Author::Run { repository: 1, number: 42 },
                references: Box::new([notes::Reference { repository: 0, number: 7 }]),
                body: b"all done".as_slice().into(),
            },
        }]),
        failed: 1,
    }
}
fn call() -> engine::Call {
    engine::Call::Read(forge::Read::Remarks { item: forge::Item { repository: 1, number: 42 }, review: 7, page: 3 })
}

#[test]
fn every_worker_origin_keeps_its_channel_for_fleet_ownership_checks() {
    let run = names::run(ITEM).expect("fits").raw();
    let attempt = names::attempt(ITEM, 3).expect("fits").raw();
    let body = payload::encode_call(call(), &SIZES).expect("fits");
    let fact = payload::encode_fact(views::Kind::Text, b"said".as_slice().into(), &SIZES).expect("fits");
    for message in [
        wire::Message::Relay { run, attempt, call: 4, body },
        wire::Message::Bounced { run, attempt, event: 5, bounce: wire::Bounce::Full },
        wire::Message::Told { run, attempt, fact },
        wire::Message::Rejected { run, attempt, account: 7, generation: 2 },
        wire::Message::Exhausted { run, attempt, account: 7, retry_after: Duration::from_secs(5) },
    ] {
        let event = translate::up(CHANNEL, message, 2, &SIZES).expect("valid").expect("a worker input");
        let channel = match event {
            engine::Event::Relay { channel, .. }
            | engine::Event::Bounced { channel, .. }
            | engine::Event::Told { channel, .. }
            | engine::Event::Rejected { channel, .. }
            | engine::Event::Exhausted { channel, .. } => channel,
            event @ (engine::Event::Refreshed { .. }
            | engine::Event::RefreshFailed { .. }
            | engine::Event::Answered { .. }
            | engine::Event::Hint { .. }
            | engine::Event::Hello { .. }
            | engine::Event::Lost { .. }
            | engine::Event::Answer { .. }
            | engine::Event::Ask { .. }
            | engine::Event::Unwatch { .. }
            | engine::Event::Delivered { .. }
            | engine::Event::Stored { .. }) => panic!("an origin input: {event:?}"),
        };
        assert_eq!(channel, CHANNEL);
    }
}

#[test]
fn relay_answers_inbound_facts_and_source_timestamps_use_the_single_payload_schema() {
    let bytes = payload::encode_call(call(), &SIZES).expect("fits");
    assert_eq!(payload::decode_call(&bytes, &SIZES), Some(call()));
    let bytes = payload::encode_served(served(), &SIZES).expect("fits");
    assert_eq!(payload::decode_served(&bytes, &SIZES), Some(served()));
    let inbound = engine::Inbound::News(forge::News::Pull {
        commit: [1; 32],
        ci: forge::Ci::Failed,
        open: true,
        merged: None,
        mergeable: false,
    });
    let bytes = payload::encode_inbound(inbound, &SIZES).expect("fits");
    assert_eq!(payload::decode_inbound(&bytes, &SIZES), Some(inbound));
    let bytes = payload::encode_fact(views::Kind::Text, b"hello".as_slice().into(), &SIZES).expect("fits");
    assert_eq!(payload::decode_fact(&bytes, &SIZES), Some((views::Kind::Text, b"hello".as_slice().into())));
    let make = || {
        engine::Served::Read(forge::api::Answer::Items { items: Box::new([]), more: true, now: Time::from_nanos(123) })
    };
    let bytes = payload::encode_served(make(), &SIZES).expect("fits");
    assert_eq!(payload::decode_served(&bytes, &SIZES), Some(make()));
    assert_eq!(payload::decode_served(&bytes[..bytes.len() - 1], &SIZES), None);
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert_eq!(payload::decode_served(&trailing, &SIZES), None);
}

#[test]
fn malformed_terminal_outcomes_preserve_landed_work_and_malformed_relays_get_named_refusals() {
    let run = names::run(ITEM).expect("fits").raw();
    let attempt = names::attempt(ITEM, 3).expect("fits").raw();
    let terminal = wire::Message::Answer {
        run,
        attempt,
        answer: wire::LinkAnswer::Ended {
            outcome: b"malformed outcome".as_slice().into(),
            work: wire::Work { landed: Box::new([wire::Landed { tag: 1, commit: [4; 32] }]), saved: None },
        },
    };
    let expected = engine::Event::Answer {
        channel: CHANNEL,
        item: ITEM,
        attempt: 3,
        answer: engine::Answer::Failed {
            failure: engine::Failure::Agent,
            work: engine::Work { landed: Box::new([engine::Landed { repository: 1, commit: [4; 32] }]) },
        },
    };
    assert_eq!(translate::up(CHANNEL, terminal, 2, &SIZES), Ok(Some(expected)));
    let relay = wire::Message::Relay { run, attempt, call: 77, body: b"bad call".as_slice().into() };
    let error = translate::up(CHANNEL, relay, 2, &SIZES).expect_err("no malformed call enters the domain");
    assert_eq!(error, Error::InvalidRelay { run, attempt, call: 77 });
    let reply = translate::reply(error, &SIZES).expect("malformed relays are answered directly");
    let bytes = codec::encode(&reply, &SIZES).expect("reply fits");
    let Some(wire::Message::Relayed { run: echoed_run, attempt: echoed_attempt, call, answer }) =
        codec::decode(&bytes, &SIZES)
    else {
        panic!("a relayed answer")
    };
    assert_eq!((echoed_run, echoed_attempt, call), (run, attempt, 77));
    assert_eq!(payload::decode_served(&answer, &SIZES), Some(engine::Served::Unserved(engine::Unserved::Invalid)));
}
