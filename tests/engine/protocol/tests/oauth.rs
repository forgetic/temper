use skein_io as io;
use skein_lib::{Duration, Time, Token, Wall};
use temper_engine_domain::{self as engine, accounts};
use temper_engine_protocol::{
    credentials::{self, Initial, Table},
    oauth::{self, Event},
};
use temper_engine_protocol_world::oauth::{DOCUMENTS, LIMITS, World, config, refresh};

fn begin(world: &mut World) -> Token {
    world.request(accounts::Request::Refresh { account: 1, generation: 5 });
    let operation = world.owner.operation(1).expect("refresh active");
    world.connected(operation);
    operation
}
fn failed(world: &World, expected: accounts::Failure) {
    assert!(
        world
            .notices
            .iter()
            .any(|event| matches!(event, engine::Event::RefreshFailed {failure, ..} if *failure == expected))
        , "expected {expected:?}; notices={:?}; records={}; ready={}", world.notices, world.records.len(), world.owner.is_ready()
    );
}

#[test]
fn candidate_waits_for_durable_keeper_and_validity_ages_during_save() {
    let mut world = World::new();
    let operation = begin(&mut world);
    world.success(operation, 30);
    assert_eq!(world.records.len(), 1);
    assert!(world.owner.table().values().is_empty());
    assert!(world.notices.is_empty());
    assert!(world.peers[&operation].aborted);
    world.now = Time::from_nanos(4_000_000_000);
    world.wall = Wall::from_nanos(900_000_000_000);
    world.up(Event::Kept { owner: operation });
    assert!(
        matches!(world.notices.as_slice(), [engine::Event::Refreshed {account: 1, generation: 5, valid}] if *valid == Duration::from_secs(26))
    );
    let values = world.owner.table().values();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].token.as_ref(), b"access-5");
    assert_eq!(values[0].expires, Time::from_nanos(30_000_000_000));
    assert_eq!(world.owner.bindings(), 1, "protocol terminal does not release the socket");
    world.closed(operation);
    assert_eq!(world.owner.bindings(), 0);
    world.up(Event::Kept { owner: operation });
    assert_eq!(world.notices.len(), 1);
}
#[test]
fn save_failure_and_save_timeout_retry_only_the_identical_kept_record() {
    for timeout in [false, true] {
        let mut world = World::new();
        let operation = begin(&mut world);
        world.success(operation, 30);
        if timeout {
            world.fire(5);
            assert_eq!(world.cancelled_keeps, [operation]);
            world.up(Event::KeepCancelled { owner: operation });
        } else {
            world.up(Event::Unkept { owner: operation });
        }
        failed(&world, accounts::Failure::Unsaved { valid: Duration::from_secs(if timeout { 25 } else { 30 }) });
        world.closed(operation);
        world.request(accounts::Request::Keep { account: 1, generation: 5 });
        let retry = world.owner.operation(1).expect("keeper retry");
        assert_ne!(retry, operation);
        assert_eq!(world.peers.len(), 1, "Keep never creates another HTTP attempt");
        assert_eq!(world.records[0].3, world.records[1].3);
        world.up(Event::Kept { owner: operation });
        assert!(world.owner.table().values().is_empty(), "stale keeper token cannot settle a new Keep");
        world.up(Event::Kept { owner: retry });
        assert_eq!(world.owner.table().values()[0].generation, 5);
    }
}
#[test]
fn socket_cancellation_waits_for_actual_closed_even_before_connecting() {
    for connected in [false, true] {
        let mut world = World::new();
        world.request(accounts::Request::Refresh { account: 1, generation: 5 });
        let operation = world.owner.operation(1).expect("refresh");
        if connected {
            world.connected(operation);
        }
        world.request(accounts::Request::Cancel { account: 1, generation: 5 });
        assert!(world.notices.is_empty());
        assert_eq!(world.owner.operation(1), Some(operation));
        assert_eq!(world.owner.bindings(), 1);
        if !connected {
            world.connected(operation);
        }
        assert!(world.peers[&operation].aborted);
        world.up(Event::Io(io::Event::Failed { owner: operation, error: io::Error::Reset }));
        assert!(world.notices.is_empty());
        world.closed(operation);
        failed(&world, accounts::Failure::Cancelled);
        assert_eq!(world.owner.operation(1), None);
        assert_eq!(world.owner.bindings(), 0);
        world.closed(operation);
        assert_eq!(world.notices.len(), 1);
    }
}
#[test]
fn keeper_cancel_race_installs_durable_rotation_before_reporting_cancelled() {
    let mut world = World::new();
    let operation = begin(&mut world);
    world.success(operation, 30);
    world.request(accounts::Request::Cancel { account: 1, generation: 5 });
    assert!(world.notices.is_empty());
    world.up(Event::Kept { owner: operation });
    failed(&world, accounts::Failure::Cancelled);
    assert_eq!(world.owner.table().values()[0].generation, 5);
    let request = world.owner.table().request(1, 6).expect("future generation uses durable rotation");
    assert_eq!(request.refresh_token.as_ref(), b"refresh-5");
    world.closed(operation);
    assert_eq!(world.owner.bindings(), 0);
}
#[test]
fn compact_unknown_length_and_early_final_errors_keep_their_classification() {
    for unknown in [false, true] {
        for (status, body, retry, expected) in [
            (400, br#"{"error":"invalid_grant"}"#.as_slice(), None, accounts::Failure::Refused),
            (503, br#"{"error":"server_error"}"#, None, accounts::Failure::TimedOut),
            (
                429,
                b"malformed",
                Some(b"13".as_slice()),
                accounts::Failure::RateLimited { retry_after: Duration::from_secs(13) },
            ),
        ] {
            let mut world = World::new();
            let operation = begin(&mut world);
            world.response(operation, status, body, retry, unknown);
            failed(&world, expected);
            assert!(world.records.is_empty());
            world.closed(operation);
        }
    }
}
#[test]
fn final_rejection_before_upload_finish_preserves_the_oauth_error() {
    let mut world = World::new();
    world.request(accounts::Request::Refresh { account: 1, generation: 5 });
    let operation = world.owner.operation(1).expect("refresh");
    world.peers.get_mut(&operation).expect("connecting peer").blocked_body = true;
    world.connected(operation);
    assert!(world.peers[&operation].sent.ends_with(b"\r\n\r\n"), "head was sent while body room remains withheld");
    world.response(operation, 403, br#"{"error":"invalid_grant"}"#, None, false);
    failed(&world, accounts::Failure::Refused);
    assert!(world.records.is_empty());
}
#[test]
fn timeout_is_unavailable_only_when_no_request_send_was_handed_out() {
    for before_send in [false, true] {
        let mut world = World::new();
        world.request(accounts::Request::Refresh { account: 1, generation: 5 });
        let operation = world.owner.operation(1).expect("refresh");
        let peer = world.peers.get_mut(&operation).expect("peer");
        peer.blocked_head = before_send;
        peer.blocked_body = !before_send;
        world.connected(operation);
        assert_eq!(world.peers[&operation].sent.is_empty(), before_send);
        world.fire(5);
        failed(&world, if before_send { accounts::Failure::Unavailable } else { accounts::Failure::TimedOut });
        assert_eq!(world.owner.bindings(), 1);
        world.closed(operation);
        assert_eq!(world.owner.bindings(), 0);
    }
}
#[test]
fn unsaved_rotation_keeps_only_the_old_granted_value_until_its_actual_expiry() {
    let record = temper_oauth::SavedToken {
        account: 1,
        generation: 4,
        access_token: b"access-4".as_slice().into(),
        refresh_token: b"refresh-4".as_slice().into(),
        account_id: None,
        expires_at: Wall::from_nanos(10_000_000_000),
    };
    let record = temper_oauth::encode_record(&record, &DOCUMENTS).expect("saved initial");
    let mut world = World::with(config(Initial::Saved { record }), LIMITS);
    let operation = begin(&mut world);
    world.success(operation, 30);
    world.up(Event::Unkept { owner: operation });
    assert_eq!(world.owner.table().values().len(), 1);
    assert_eq!(world.owner.table().values()[0].generation, 4);
    assert_eq!(world.owner.table().values()[0].token.as_ref(), b"access-4");
    world.fire(10);
    assert!(world.owner.table().values()[0].token.is_empty());
    assert_eq!(world.owner.table().candidate_valid(1, 5, world.now), Ok(Duration::from_secs(20)));
}
#[test]
fn real_account_domain_retries_keep_after_a_save_timeout_without_a_second_post() {
    use skein_lib::{Env, Queue};
    let limits = accounts::Limits {
        accounts: 1,
        refresh_margin: Duration::from_secs(2),
        backoff_base: Duration::from_secs(1),
        backoff_max: Duration::from_secs(4),
        rejected_interval: Duration::from_secs(1),
        spent_attention: Duration::from_secs(10),
        facts: 4,
    };
    for timeout in [false, true] {
        let mut world = World::new();
        let mut domain = accounts::Domain::new(&limits);
        let mut requests = Queue::with_capacity(accounts::MAX_OUT);
        let initial = world.owner.table().metadata()[0];
        let mut env = Env { now: world.now, wall: world.wall, limits };
        accounts::step(
            &mut domain,
            &env,
            accounts::Event::Add { account: initial.account, generation: initial.generation, valid: initial.valid },
            &mut requests,
        );
        world.request(requests.pop().expect("refresh-only starts immediately"));
        let operation = world.owner.operation(1).expect("refresh");
        world.connected(operation);
        world.success(operation, 30);
        if timeout {
            world.fire(5);
            world.up(Event::KeepCancelled { owner: operation });
        } else {
            world.up(Event::Unkept { owner: operation });
        }
        world.closed(operation);
        let engine::Event::RefreshFailed { account, generation, failure } =
            world.notices.pop().expect("unsaved terminal")
        else {
            panic!("refresh failed");
        };
        assert!(matches!(failure, accounts::Failure::Unsaved { .. }));
        env.now = world.now;
        accounts::step(&mut domain, &env, accounts::Event::Failed { account, generation, failure }, &mut requests);
        world.now = world.now.saturating_add(Duration::from_secs(1));
        env.now = world.now;
        accounts::fire(&mut domain, &env, &mut requests);
        let retry = requests.pop().expect("backoff emits keeper retry");
        assert!(matches!(retry, accounts::Request::Keep { account: 1, generation: 5 }));
        world.request(retry);
        assert_eq!(world.peers.len(), 1, "domain and executor agree no second OAuth request");
        let retry = world.owner.operation(1).expect("keeper retry");
        world.up(Event::Kept { owner: retry });
        let engine::Event::Refreshed { account, generation, valid } =
            world.notices.pop().expect("durable refreshed terminal")
        else {
            panic!("refresh succeeded");
        };
        accounts::step(&mut domain, &env, accounts::Event::Refreshed { account, generation, valid }, &mut requests);
        assert!(domain.usable(1));
    }
}
#[test]
fn response_completion_time_and_retry_http_date_use_injected_clocks() {
    let mut world = World::new();
    let operation = begin(&mut world);
    world.now = Time::from_nanos(8_000_000_000);
    world.success(operation, 20);
    world.now = Time::from_nanos(11_000_000_000);
    world.up(Event::Kept { owner: operation });
    assert_eq!(world.owner.table().values()[0].expires, Time::from_nanos(28_000_000_000));
    let mut limited = World::new();
    let operation = begin(&mut limited);
    limited.wall = Wall::from_nanos(10_000_000_000);
    limited.response(operation, 429, br#"{"error":"slow_down"}"#, Some(b"Thu, 01 Jan 1970 00:00:23 GMT"), false);
    failed(&limited, accounts::Failure::RateLimited { retry_after: Duration::from_secs(13) });
}
#[test]
fn bootstrap_generation_and_absolute_expiry_survive_restart_and_wall_changes() {
    let record = temper_oauth::SavedToken {
        account: 1,
        generation: 22,
        access_token: b"kept-access".as_slice().into(),
        refresh_token: b"kept-refresh".as_slice().into(),
        account_id: None,
        expires_at: Wall::from_nanos(90_000_000_000),
    };
    let bytes = temper_oauth::encode_record(&record, &DOCUMENTS).expect("saved record");
    let mut table = Table::new(
        Box::new([config(Initial::Saved { record: bytes })]),
        Box::new([]),
        Time::from_nanos(4_000_000_000),
        Wall::from_nanos(80_000_000_000),
        &LIMITS.credentials,
    )
    .expect("bootstrap");
    assert_eq!(table.metadata()[0].generation, 22);
    assert_eq!(table.metadata()[0].valid, Some(Duration::from_secs(10)));
    assert_eq!(table.values()[0].expires, Time::from_nanos(14_000_000_000));
    assert_eq!(table.validate_refresh(1, 22), Err(credentials::Error::Generation));
    assert!(table.validate_refresh(1, 23).is_ok());
    table.expire(Time::from_nanos(14_000_000_000));
    assert!(table.values()[0].token.is_empty());
    let exhausted = Table::new(
        Box::new([config(Initial::Refresh { generation: u64::MAX, token: b"refresh".as_slice().into() })]),
        Box::new([]),
        Time::ZERO,
        Wall::EPOCH,
        &LIMITS.credentials,
    );
    assert!(matches!(exhausted, Err(credentials::Error::Generation)));
}
#[test]
fn two_generations_are_retained_and_oldest_is_evicted_without_reusing_names() {
    let mut table =
        Table::new(Box::new([config(refresh())]), Box::new([]), Time::ZERO, Wall::EPOCH, &LIMITS.credentials)
            .expect("bootstrap");
    for generation in 5..9 {
        table
            .prepare(
                1,
                generation,
                temper_oauth::TokenResponse {
                    access_token: format!("access-{generation}").into_bytes().into(),
                    refresh_token: None,
                    expires_in: 20,
                },
                Time::ZERO,
                Wall::EPOCH,
                &LIMITS.credentials,
            )
            .expect("candidate");
        table.saved(1, generation, Time::ZERO).expect("durable");
        assert!(table.values().len() <= 2);
    }
    assert_eq!(
        table.values().iter().map(|value| value.generation).collect::<std::collections::BTreeSet<_>>(),
        [7, 8].into()
    );
    assert_eq!(
        table.request(1, 9).expect("omitted refresh preserves durable prior").refresh_token.as_ref(),
        b"refresh-4"
    );
}
#[test]
fn chatgpt_claims_cap_exchange_validity_and_bad_metadata_never_reaches_keeper() {
    const JWT: &[u8] =
        b"e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC03In0sImV4cCI6MTIwfQ.AA";
    let mut startup = config(refresh());
    startup.kind = temper_oauth::AccountKind::ChatGpt;
    let mut world = World::with(startup, LIMITS);
    let operation = begin(&mut world);
    world.wall = Wall::from_nanos(115_000_000_000);
    let body = temper_oauth::encode_response(
        &temper_oauth::TokenResponse { access_token: JWT.into(), refresh_token: None, expires_in: 30 },
        &DOCUMENTS,
    )
    .expect("token response");
    world.response(operation, 200, &body, None, false);
    world.up(Event::Kept { owner: operation });
    assert_eq!(world.owner.table().values()[0].account_id.as_ref(), b"acct-7");
    assert_eq!(world.owner.table().values()[0].expires, Time::from_nanos(5_000_000_000));
    for access in [b"opaque".as_slice(), JWT] {
        let mut startup = config(refresh());
        startup.kind = temper_oauth::AccountKind::ChatGpt;
        let mut invalid = World::with(startup, LIMITS);
        let operation = begin(&mut invalid);
        invalid.wall = Wall::from_nanos(121_000_000_000);
        let body = temper_oauth::encode_response(
            &temper_oauth::TokenResponse { access_token: access.into(), refresh_token: None, expires_in: 30 },
            &DOCUMENTS,
        )
        .expect("bounded response");
        invalid.response(operation, 200, &body, None, false);
        failed(&invalid, accounts::Failure::TimedOut);
        assert!(invalid.records.is_empty());
        assert!(invalid.owner.table().values().is_empty());
    }
}
#[test]
fn reaching_max_generation_is_allowed_once_and_no_next_name_can_wrap() {
    let mut table = Table::new(
        Box::new([config(Initial::Refresh { generation: u64::MAX - 1, token: b"refresh".as_slice().into() })]),
        Box::new([]),
        Time::ZERO,
        Wall::EPOCH,
        &LIMITS.credentials,
    )
    .expect("last refresh admitted");
    table
        .prepare(
            1,
            u64::MAX,
            temper_oauth::TokenResponse {
                access_token: b"last-access".as_slice().into(),
                refresh_token: None,
                expires_in: 10,
            },
            Time::ZERO,
            Wall::EPOCH,
            &LIMITS.credentials,
        )
        .expect("last named candidate");
    table.saved(1, u64::MAX, Time::ZERO).expect("durable last generation");
    assert_eq!(table.values()[0].generation, u64::MAX);
    assert_eq!(table.validate_refresh(1, 0), Err(credentials::Error::Generation));
}
#[test]
fn tight_io_and_credential_entrances_refuse_before_operations() {
    let mut limits = LIMITS;
    limits.operations = 0;
    assert!(oauth::worst_case(&limits).is_none());
    let mut io = temper_engine_protocol_world::oauth::io_limits();
    io.intake = 3;
    assert!(!oauth::fits_io(&LIMITS, &io));
    let mut config = config(refresh());
    config.endpoint.transport = temper_engine_protocol::connection::Transport::Loopback;
    config.endpoint.address = "192.0.2.1:80".parse().expect("fixed address");
    assert!(Table::new(Box::new([config]), Box::new([]), Time::ZERO, Wall::EPOCH, &LIMITS.credentials).is_err());
}
