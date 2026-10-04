use skein_lib::{Duration, Queue, Time, Token};
use temper_channel::payload;
use temper_engine_protocol::names;
use temper_engine_protocol::{listener::Notice, translate::Value};
use temper_legacy_engine_domain as engine;
use temper_worker_domain as worker;
use temper_worker_protocol::link;
use temper_worker_protocol_world::sockets::{LIMITS, OWNER, World};

const ITEM: engine::Item = engine::Item { repository: 1, number: 42 };
fn values() -> [Value; 2] {
    [
        Value {
            account: 1,
            generation: 7,
            expires: Time::from_nanos(100_000_000_000),
            token: b"oauth".as_slice().into(),
            account_id: b"user".as_slice().into(),
        },
        Value {
            account: 2,
            generation: 0,
            expires: Time::from_nanos(u64::MAX),
            token: b"git-static".as_slice().into(),
            account_id: Box::new([]),
        },
    ]
}
fn assignment() -> engine::Assignment {
    engine::Assignment {
        item: ITEM,
        attempt: 3,
        grants: Box::new([
            engine::accounts::Grant { account: 1, generation: 7, valid: Duration::from_secs(50) },
            engine::accounts::Grant { account: 2, generation: 0, valid: Duration::from_secs(50) },
        ]),
        workspace: engine::Workspace {
            key: b"work".as_slice().into(),
            repositories: Box::new([engine::Checkout {
                repository: 1,
                start: engine::Start::Base { branch: b"main".as_slice().into() },
                push: Some(b"work".as_slice().into()),
            }]),
        },
        save: Some(b"saved".as_slice().into()),
        charter: temper_engine_protocol_world::charter(),
        snapshot: Some(b"\xffopaque snapshot".as_slice().into()),
    }
}
#[expect(clippy::too_many_lines, reason = "one ordered socket loss/reconnect scenario with both referees")]
fn exercise(seed: u64, faults: bool) -> String {
    let mut world = World::new(seed, faults);
    let channel = world.channel();
    world.engine(engine::Request::Assign { channel, assignment: assignment() }, &values());
    world.settle();
    let received = world
        .events
        .iter()
        .find(|event| matches!(event, worker::Event::Assign { .. }))
        .expect("assignment crossed actual sockets");
    let worker::Event::Assign { assignment: received } = received else {
        panic!("Assign");
    };
    assert_eq!(received.snapshot.as_deref(), Some(b"\xffopaque snapshot".as_slice()));
    assert_eq!(received.grants.len(), 2);
    assert_eq!(received.workspace.repositories[0].tag, 1);
    assert_eq!(received.workspace.repositories[0].identity, 9);
    assert!(world.link.credentials().grant(1, 7, world.sim.now()).is_some());
    let run = names::run(ITEM).expect("run");
    let attempt = names::attempt(ITEM, 3).expect("attempt");
    let call = Token::new(333);
    let body = payload::encode_call(&engine_call(), &LIMITS.sizes).expect("call");
    world.down(worker::Request::Relay { run, attempt, call, body });
    world.settle();
    assert!(
        world
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::Domain(engine::Event::Relay { call: found, .. }) if *found == call))
    );
    world.hold_closed = true;
    world.engine(engine::Request::Refuse { channel }, &[]);
    world.settle();
    assert_eq!(world.link.state(), link::State::Closing);
    assert_eq!(world.link.relays().len(), 1, "relay wait survives transport loss");
    assert!(!world.held_closed.is_empty());
    let mut up = Queue::with_capacity(link::MAX_UP);
    let mut down = Queue::with_capacity(link::MAX_DOWN);
    let env = skein_lib::Env { now: world.sim.now(), wall: skein_lib::Wall::EPOCH, limits: LIMITS };
    assert_eq!(link::down(&mut world.link, &env, worker::Request::Dial, &mut up, &mut down), Err(link::Error::State));
    assert!(!world.events.iter().any(|event| matches!(event, worker::Event::Lost)));
    world.hold_closed = false;
    world.release_closed();
    assert_eq!(world.link.state(), link::State::Idle);
    assert_eq!(world.events.iter().filter(|event| matches!(event, worker::Event::Lost)).count(), 1);
    world.down(worker::Request::Dial);
    world.settle();
    let replacement = world.channel();
    assert_ne!(replacement, channel);
    world.engine(
        engine::Request::Relayed {
            channel: replacement,
            item: ITEM,
            attempt: 3,
            call,
            served: engine::Served::Posted { comment: 17 },
        },
        &[],
    );
    world.settle();
    assert!(
        world.events.iter().any(|event| matches!(event, worker::Event::Relayed { call: found, .. } if *found == call))
    );
    assert!(world.link.relays().is_empty());
    let cancelled = Token::new(444);
    world.down(worker::Request::Relay {
        run,
        attempt,
        call: cancelled,
        body: payload::encode_call(&engine_call(), &LIMITS.sizes).expect("call"),
    });
    world.settle();
    world.down(worker::Request::CancelRelay { call: cancelled });
    world.engine(
        engine::Request::Relayed {
            channel: replacement,
            item: ITEM,
            attempt: 3,
            call: cancelled,
            served: engine::Served::Posted { comment: 18 },
        },
        &[],
    );
    world.settle();
    assert_eq!(
        world
            .events
            .iter()
            .filter(|event| matches!(event, worker::Event::RelayCancelled { call } if *call == cancelled))
            .count(),
        1
    );
    assert!(
        !world.events.iter().any(|event| matches!(event, worker::Event::Relayed { call, .. } if *call == cancelled))
    );
    world.engine(
        engine::Request::Inbound {
            channel: replacement,
            item: ITEM,
            attempt: 3,
            name: Token::new(9001),
            event: engine::Inbound::Decided { accepted: true },
        },
        &[],
    );
    world.engine(engine::Request::Cancel { channel: replacement, item: ITEM, attempt: 3 }, &[]);
    world.settle();
    assert!(
        world.events.iter().any(|event| matches!(event, worker::Event::Inbound { name, .. } if name.raw() == 9001))
    );
    assert!(
        world.events.iter().any(|event| matches!(event, worker::Event::Cancel { run: found, .. } if *found == run))
    );
    world.shutdown();
    let _ = OWNER;
    world.sim.render_trace()
}
fn engine_call() -> payload::Call {
    payload::Call::Comment { text: b"hello".as_slice().into() }
}
#[test]
fn production_links_fragment_reconnect_and_cancel() {
    exercise(19, false);
}
#[test]
fn short_socket_completions_replay() {
    assert_eq!(exercise(97, true), exercise(97, true));
}

#[test]
fn authenticated_replacement_waits_for_old_actual_io_closed_before_accept() {
    let mut world = World::new(33, false);
    let old = world.channel();
    world.hold_engine_closed = true;
    world.replace();
    world.settle();
    assert!(!world.held_engine_closed.is_empty(), "real old io has supplied Closed, queued upward");
    assert_eq!(world.channel(), old);
    assert_eq!(world.replacement.as_ref().expect("replacement").link.state(), link::State::Opening);
    assert!(
        !world
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::Domain(engine::Event::Lost { channel }) if *channel == old))
    );
    assert!(
        !world
            .replacement
            .as_ref()
            .expect("replacement")
            .events
            .iter()
            .any(|event| matches!(event, worker::Event::Connected))
    );
    world.hold_engine_closed = false;
    world.release_engine_closed();
    world.settle();
    let new = world.channel();
    assert_ne!(old, new);
    assert_eq!(world.replacement.as_ref().expect("replacement").link.state(), link::State::Open);
    let lost = world
        .notices
        .iter()
        .position(|notice| matches!(notice, Notice::Domain(engine::Event::Lost { channel }) if *channel == old))
        .expect("old Lost");
    let hello = world
        .notices
        .iter()
        .position(|notice| matches!(notice, Notice::Domain(engine::Event::Hello { channel, .. }) if *channel == new))
        .expect("new Hello");
    assert!(lost < hello);
    world.engine(engine::Request::Assign { channel: new, assignment: assignment() }, &values());
    world.settle();
    assert!(
        world
            .replacement
            .as_ref()
            .expect("replacement")
            .events
            .iter()
            .any(|event| matches!(event, worker::Event::Assign { .. }))
    );
    world.shutdown();
}

#[test]
fn rejected_unknown_grant_closed_before_resume_cannot_poison_a_new_dial() {
    let mut world = World::new(55, false);
    let channel = world.channel();
    world.hold_worker_ready = true;
    let grant = engine::accounts::Grant { account: 3, generation: 7, valid: Duration::from_secs(50) };
    let value = Value {
        account: 3,
        generation: 7,
        expires: Time::from_nanos(100_000_000_000),
        token: b"unknown account".as_slice().into(),
        account_id: Box::new([]),
    };
    world.engine(engine::Request::Grant { channel, item: ITEM, attempt: 3, grant }, &[value]);
    world.settle();
    assert_eq!(world.link.state(), link::State::Idle, "actual Closed retires binding without a child resume");
    assert!(!world.link.is_ready(), "old child terminal/output scratch has been discarded");
    world.hold_worker_ready = false;
    world.down(worker::Request::Dial);
    world.settle();
    assert_eq!(world.link.state(), link::State::Open);
    world.shutdown();
}
