use skein_lib::{Duration, Time, Token};
use temper_channel::wire;
use temper_worker_domain::{self as worker, agent::channel};
use temper_worker_protocol::agent;
use temper_worker_protocol_world::agent::{World, grant, start};

#[test]
fn opaque_start_full_endpoint_table_and_only_llm_values() {
    let mut world = World::new();
    world.settle();
    assert_eq!(world.bridge.state(), agent::State::Ready);
    world.send(Token::new(100), start());
    world.settle();
    let wire::Message::AgentStart { charter, snapshot, endpoints, grants, repositories } = &world.messages[0] else {
        panic!("Start first");
    };
    assert_eq!(charter.as_ref(), b"\xffopaque charter");
    assert_eq!(snapshot.as_deref(), Some(b"\xfeopaque snapshot".as_slice()));
    assert_eq!(endpoints.len(), 2);
    assert_eq!(repositories[0].name.as_ref(), b"source");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].account, 1);
    assert!(world.events.iter().any(|event| matches!(event, worker::Event::Sent { owner } if owner.raw() == 100)));
    world.send(Token::new(101), channel::Down::Event { name: Token::new(9001), event: b"\xffevent".as_slice().into() });
    world.settle();
    assert!(world.messages.iter().any(
        |message| matches!(message, wire::Message::AgentEvent { event: 9001, body } if body.as_ref() == b"\xffevent")
    ));
}
#[test]
fn finish_is_delivered_and_binding_remains_through_stdout_eof() {
    let mut world = World::new();
    world.settle();
    world.send(Token::new(100), start());
    world.settle();
    world.read(Token::new(200));
    world
        .peer_send(wire::Message::Finish { finish: wire::Finish::Ended { outcome: b"\xffoutcome".as_slice().into() } });
    world.settle();
    assert!(world.events.iter().any(|event| matches!(event, worker::Event::Received { owner, message: channel::Up::Finish { finish: channel::Finish::Ended { outcome } } } if owner.raw() == 200 && outcome.as_ref() == b"\xffoutcome")));
    world.read(Token::new(201));
    world.eof();
    assert!(world.events.iter().any(|event| matches!(event, worker::Event::Hangup { owner } if owner.raw() == 201)));
    assert_eq!(world.bridge.state(), agent::State::Failed);
    assert_eq!(world.bridge.process(), Token::new(33));
    assert!(
        !world
            .events
            .iter()
            .any(|event| matches!(event, worker::Event::Unspawned { .. } | worker::Event::Reaped { .. }))
    );
    world.closed();
    assert_eq!(world.bridge.state(), agent::State::Closed);
    world.send(Token::new(202), channel::Down::Cancel);
    assert!(world.events.iter().any(|event| matches!(event, worker::Event::Unsent { owner } if owner.raw() == 202)));
}
#[test]
fn queued_start_ages_values_before_handshake_completes() {
    let mut world = World::new();
    world.block_accept(true);
    world.send(Token::new(100), start());
    world.settle();
    world.now = Time::from_nanos(4_000_000_000);
    world.block_accept(false);
    world.settle();
    let wire::Message::AgentStart { grants, .. } = &world.messages[0] else {
        panic!("Start");
    };
    assert_eq!(grants[0].valid, Duration::from_secs(6));
}
#[test]
fn pending_start_drops_expired_values_without_revealing_static_git() {
    let mut world = World::new();
    world.credentials.insert(grant(1, 7, 2), Time::ZERO, Duration::ZERO).expect("shorten");
    world.block_accept(true);
    world.send(Token::new(100), start());
    world.settle();
    world.now = Time::from_nanos(3_000_000_000);
    world.block_accept(false);
    world.settle();
    let wire::Message::AgentStart { grants, .. } = &world.messages[0] else {
        panic!("Start");
    };
    assert!(grants.is_empty());
}
#[test]
fn crossed_pending_read_and_send_terminate_once_on_failed_stream() {
    let mut world = World::new();
    world.block_accept(true);
    world.send(Token::new(100), start());
    world.read(Token::new(200));
    world.settle();
    // This fails both calls in one full public entrypoint, using exactly MAX_UP slots.
    let mut up = skein_lib::Queue::with_capacity(agent::MAX_UP);
    let mut down = skein_lib::Queue::with_capacity(agent::MAX_DOWN);
    let env = skein_lib::Env {
        now: Time::ZERO,
        wall: skein_lib::Wall::EPOCH,
        limits: temper_worker_protocol_world::sockets::LIMITS,
    };
    agent::up(
        &mut world.bridge,
        &env,
        skein_lib::stream::Up::Failed(skein_lib::stream::Fault::Reset),
        &mut up,
        &mut down,
    );
    let mut records = Vec::new();
    while let Some(event) = up.pop() {
        records.push(event);
    }
    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|event| matches!(event, worker::Event::Unsent { owner } if owner.raw() == 100)));
    assert!(records.iter().any(|event| matches!(event, worker::Event::Hangup { owner } if owner.raw() == 200)));
}
