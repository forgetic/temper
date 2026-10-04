use skein_lib::{Duration, Time};
use temper_engine_protocol::oauth::Event;
use temper_engine_protocol_world::oauth_socket::World;
use temper_fake_llm_protocol::oauth::{Body, Plan};
use temper_legacy_engine_domain::{self as engine, accounts};
fn token() -> Plan {
    Plan {
        status: 200,
        body: Body::Token(temper_oauth::TokenResponse {
            access_token: b"access-5".as_slice().into(),
            refresh_token: Some(b"refresh-5".as_slice().into()),
            expires_in: 30,
        }),
        retry_after: None,
        head_delay: Duration::ZERO,
        body_delay: Duration::ZERO,
    }
}
#[test]
fn real_socket_refresh_is_saved_before_grant_and_binding_waits_for_closed() {
    let mut world = World::new(11, true);
    world.queue(token());
    world.hold_closed = true;
    world.refresh(5);
    world.drive();
    assert_eq!(world.issuer.posts(), 1);
    assert_eq!(world.records.len(), 1);
    assert!(world.notices.is_empty());
    assert!(world.table().values().is_empty());
    let operation = world.records[0].0;
    world.event(Event::Kept { owner: operation });
    assert!(world.notices.iter().any(|event| matches!(event, engine::Event::Refreshed { generation: 5, .. })));
    world.settle();
    assert_eq!(world.owner.bindings(), 1);
    assert!(!world.held_closed.is_empty());
    world.release_closed();
    assert_eq!(world.owner.bindings(), 0);
    world.close();
}
#[test]
fn real_issuer_spends_refresh_once_while_keep_retry_makes_no_second_post() {
    let mut world = World::new(19, true);
    world.queue(token());
    world.refresh(5);
    world.drive();
    let operation = world.records[0].0;
    world.event(Event::Unkept { owner: operation });
    world.settle();
    let record = world.records[0].1.clone();
    let env = skein_lib::Env {
        now: world.sim.now(),
        wall: skein_lib::Wall::EPOCH,
        limits: temper_engine_protocol_world::oauth::LIMITS,
    };
    let mut upper = skein_lib::Queue::with_capacity(temper_engine_protocol::oauth::MAX_UP);
    let mut lower = skein_lib::Queue::with_capacity(temper_engine_protocol::oauth::MAX_OUT);
    temper_engine_protocol::oauth::down(
        &mut world.owner,
        &env,
        accounts::Request::Keep { account: 1, generation: 5 },
        &mut upper,
        &mut lower,
    )
    .expect("save retry");
    let effect = lower.pop().expect("one keeper request");
    let temper_engine_protocol::oauth::Effect::Keep { owner: retry, record: next, .. } = effect else {
        panic!("Keep only");
    };
    assert_eq!(record, next);
    assert!(lower.is_empty());
    world.event(Event::Kept { owner: retry });
    assert_eq!(world.issuer.posts(), 1);
    assert!(world.issuer.authorize(b"access-5", world.sim.now()));
    assert!(!world.issuer.authorize(b"access-5", Time::from_nanos(40_000_000_000)));
    world.close();
}
