use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use temper_engine_domain::accounts;
use temper_engine_protocol::{
    credentials::{self, Identity, Initial, Table},
    oauth::{self, Event, Owner},
};
use temper_engine_protocol_world::oauth::{DOCUMENTS, LIMITS, config, io_limits};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
fn configured(account: u32) -> credentials::Config {
    let mut config =
        config(Initial::Refresh { generation: 4, token: vec![b'r'; DOCUMENTS.token_bytes as usize].into() });
    config.account = account;
    config.client = vec![b'c'; DOCUMENTS.client_bytes as usize].into();
    config.endpoint.host = vec![b'h'; LIMITS.credentials.endpoint_bytes as usize].into();
    config.endpoint.path =
        format!("/{}", "p".repeat(LIMITS.credentials.endpoint_bytes as usize - 1)).into_bytes().into();
    config
}
#[test]
fn maximal_values_candidate_and_exact_keeper_record_fit_table_bound() {
    let meter = Meter::new();
    meter.start();
    let mut table = Table::new(
        Box::new([configured(1), configured(2)]),
        Box::new([
            Identity { account: 3, token: vec![b'g'; DOCUMENTS.token_bytes as usize].into() },
            Identity { account: 4, token: vec![b'g'; DOCUMENTS.token_bytes as usize].into() },
        ]),
        Time::ZERO,
        Wall::EPOCH,
        &LIMITS.credentials,
    )
    .expect("maximal configuration");
    for generation in 5..8 {
        for account in 1..3 {
            table
                .prepare(
                    account,
                    generation,
                    temper_oauth::TokenResponse {
                        access_token: vec![b'a'; DOCUMENTS.token_bytes as usize].into(),
                        refresh_token: Some(vec![b'r'; DOCUMENTS.token_bytes as usize].into()),
                        expires_in: 60,
                    },
                    Time::ZERO,
                    Wall::EPOCH,
                    &LIMITS.credentials,
                )
                .expect("candidate");
            if generation < 7 {
                table.saved(account, generation, Time::ZERO).expect("durable");
            }
        }
    }
    let first = table.record(1, 7, &LIMITS.credentials).expect("exact keeper bytes");
    let second = table.record(2, 7, &LIMITS.credentials).expect("exact keeper bytes");
    let measured = meter.end();
    meter.check(measured, credentials::worst_case(&LIMITS.credentials).expect("checked bound"), 1);
    assert_eq!(table.values().len(), 6);
    assert!(!first.is_empty() && !second.is_empty());
}
#[test]
fn retired_socket_attempts_and_new_blocked_http_heads_fit_owner_bound() {
    let meter = Meter::new();
    meter.start();
    let mut owner = Owner::new(
        Box::new([configured(1), configured(2)]),
        Box::new([]),
        Time::ZERO,
        Wall::EPOCH,
        &LIMITS,
        &io_limits(),
    )
    .expect("owner");
    let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS };
    let mut upper = Queue::with_capacity(oauth::MAX_UP);
    let mut lower = Queue::with_capacity(oauth::MAX_OUT);
    for _round in 0..2 {
        for account in 1..3 {
            oauth::down(
                &mut owner,
                &env,
                accounts::Request::Refresh { account, generation: 5 },
                &mut upper,
                &mut lower,
            )
            .expect("refresh");
            clear(&mut lower);
            let operation = owner.operation(account).expect("active");
            oauth::up(
                &mut owner,
                &env,
                Event::Io(skein_io::Event::Connecting {
                    owner: operation,
                    socket: Token::new(100 + u64::from(account)),
                }),
                &mut upper,
                &mut lower,
            );
            clear(&mut lower);
            oauth::up(
                &mut owner,
                &env,
                Event::Io(skein_io::Event::Connected { owner: operation }),
                &mut upper,
                &mut lower,
            );
            clear(&mut lower);
            for _step in 0..8 {
                if !owner.is_ready() {
                    break;
                }
                oauth::resume(&mut owner, &env, &mut upper, &mut lower);
                clear(&mut lower);
            }
        }
        if env.now == Time::ZERO {
            env.now = Time::ZERO.saturating_add(Duration::from_secs(5));
            for _account in 0..2 {
                oauth::fire(&mut owner, &env, &mut upper, &mut lower);
                clear(&mut lower);
                while upper.pop().is_some() {}
            }
        }
    }
    assert_eq!(owner.bindings(), LIMITS.operations);
    let measured = meter.end();
    meter.check(measured, oauth::worst_case(&LIMITS).expect("checked owner bound"), 2);
}
fn clear(queue: &mut Queue<oauth::Effect>) {
    while queue.pop().is_some() {}
}
