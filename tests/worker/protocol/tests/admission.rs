use skein_lib::{Env, Queue, Time, Token, Wall};
use temper_worker_domain::{self as worker, host};
use temper_worker_protocol::{link, translate};
use temper_worker_protocol_world::sockets::{LIMITS, OWNER, World, io_limits};

#[test]
fn tiny_fact_flood_leaves_owed_answer_and_relay_space() {
    let mut world = World::new(15, false);
    let env = Env { now: world.sim.now(), wall: Wall::EPOCH, limits: LIMITS };
    let mut upper = Queue::with_capacity(link::MAX_UP);
    let mut lower = Queue::with_capacity(link::MAX_DOWN);
    for _ in 0..LIMITS.sizes.facts {
        assert!(link::can_tell(&world.link, &LIMITS));
        assert!(
            link::told(
                &mut world.link,
                &env,
                worker::Told { run: Token::new(1), attempt: Token::new(1), fact: Box::new([]) },
                &mut upper,
                &mut lower
            )
            .expect("fact")
        );
        while lower.pop().is_some() {}
    }
    assert!(!link::can_tell(&world.link, &LIMITS));
    assert!(
        !link::told(
            &mut world.link,
            &env,
            worker::Told { run: Token::new(1), attempt: Token::new(1), fact: Box::new([]) },
            &mut upper,
            &mut lower
        )
        .expect("no fact reserve")
    );
    assert!(lower.is_empty());
    link::down(
        &mut world.link,
        &env,
        worker::Request::Answer {
            run: Token::new(1),
            attempt: Token::new(1),
            answer: host::Answer::Ended {
                outcome: vec![0xff; LIMITS.sizes.outcome as usize].into(),
                work: host::Work { landed: Box::new([]), saved: None },
            },
        },
        &mut upper,
        &mut lower,
    )
    .expect("owed answer fits");
    assert!(lower.pop().is_some());
    while lower.pop().is_some() {}
    link::down(
        &mut world.link,
        &env,
        worker::Request::Relay {
            run: Token::new(1),
            attempt: Token::new(1),
            call: Token::new(44),
            body: vec![0xff; LIMITS.sizes.call as usize].into(),
        },
        &mut upper,
        &mut lower,
    )
    .expect("owed relay fits");
    assert!(lower.pop().is_some());
    assert_eq!(world.link.state(), link::State::Open);
}
#[test]
fn entrance_rejects_empty_secrets_and_mismatched_bounds() {
    let mut limits = LIMITS;
    limits.sizes.secret_bytes = 1;
    let config = link::Config {
        address: temper_engine_protocol_world::link::ADDR,
        name: b"worker".as_slice().into(),
        secret: b"secret".as_slice().into(),
        accounts: Box::new([]),
    };
    assert!(matches!(link::Link::new(OWNER, config, &limits, &io_limits()), Err(link::Error::Config)));
    let config = link::Config {
        address: temper_engine_protocol_world::link::ADDR,
        name: b"worker".as_slice().into(),
        secret: Box::new([]),
        accounts: Box::new([]),
    };
    assert!(matches!(link::Link::new(OWNER, config, &LIMITS, &io_limits()), Err(link::Error::Config)));
}
#[test]
fn unrepresented_grant_refusal_remains_an_explicit_projection_error() {
    assert_eq!(
        translate::link::down(
            worker::Request::Answer {
                run: Token::new(1),
                attempt: Token::new(1),
                answer: host::Answer::Refused(host::Refusal::Invalid(host::Invalid::Grants))
            },
            &LIMITS.sizes
        ),
        Err(translate::Error::Unsupported)
    );
}
#[test]
fn wrong_direction_and_excess_arrays_are_rejected_before_projection() {
    assert_eq!(
        translate::agent::up(temper_channel::wire::Message::AgentCancel, &LIMITS.sizes),
        Err(translate::Error::Direction)
    );
    assert_eq!(
        translate::link::up(temper_channel::wire::Message::AgentCancel, &LIMITS.sizes).err(),
        Some(translate::Error::Direction)
    );
    let hello = worker::Hello {
        slots: 1,
        workstreams: Box::new([]),
        hosting: (0..=LIMITS.sizes.slots)
            .map(|_| worker::Hosted { run: Token::new(1), attempt: Token::new(1), phase: worker::Phase::Active })
            .collect(),
    };
    assert_eq!(translate::link::down(worker::Request::Hello { hello }, &LIMITS.sizes), Err(translate::Error::Limits));
}
#[test]
fn failed_dial_waits_for_actual_closed_and_ignores_idle_stale_records() {
    let mut link = link::Link::new(
        OWNER,
        link::Config {
            address: temper_engine_protocol_world::link::ADDR,
            name: b"alpha".as_slice().into(),
            secret: b"one".as_slice().into(),
            accounts: Box::new([1, 2]),
        },
        &LIMITS,
        &io_limits(),
    )
    .expect("link");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS };
    let mut upper = Queue::with_capacity(link::MAX_UP);
    let mut lower = Queue::with_capacity(link::MAX_DOWN);
    link::up(
        &mut link,
        &env,
        skein_io::Event::Failed { owner: OWNER, error: skein_io::Error::Busy },
        &mut upper,
        &mut lower,
    );
    assert_eq!(link.state(), link::State::Idle);
    link::down(&mut link, &env, worker::Request::Dial, &mut upper, &mut lower).expect("dial");
    while lower.pop().is_some() {}
    link::up(
        &mut link,
        &env,
        skein_io::Event::Failed { owner: OWNER, error: skein_io::Error::Busy },
        &mut upper,
        &mut lower,
    );
    assert_eq!(link.state(), link::State::Closing);
    assert!(upper.is_empty());
    assert_eq!(link::down(&mut link, &env, worker::Request::Dial, &mut upper, &mut lower), Err(link::Error::State));
    link::up(&mut link, &env, skein_io::Event::Closed { owner: OWNER }, &mut upper, &mut lower);
    assert_eq!(upper.pop(), Some(worker::Event::Lost));
    assert_eq!(link.state(), link::State::Idle);
    link::up(&mut link, &env, skein_io::Event::Closed { owner: OWNER }, &mut upper, &mut lower);
    assert!(upper.is_empty());
}

#[test]
fn blocked_ping_attempt_moves_its_deadline_and_never_consumes_owed_reserve() {
    use temper_worker_protocol_world::byte_link::{LIMITS as TINY, World as Bytes};
    let mut world = Bytes::new();
    world.block_room();
    let cap = temper_channel::sizes::output_cap(temper_channel::machine::Endpoint::WorkerLink, &TINY.sizes)
        .expect("tiny cap");
    let mut blocked = false;
    for nanos in 1..=u64::from(cap / 8 + 2) {
        world.now = Time::from_nanos(nanos);
        world.incoming_ping();
        let before = world.pings;
        world.fire();
        assert!(world.link.next_deadline(&TINY).expect("positive timer") > world.now);
        if world.pings == before {
            let records = world.sent_records;
            world.fire();
            world.fire();
            assert_eq!(world.sent_records, records, "a blocked due ping cannot repeat at one instant");
            blocked = true;
            break;
        }
    }
    assert!(blocked, "whole-cap credit was consumed by legal tiny frames");
    assert!(
        world.sent_records
            <= temper_channel::sizes::stream_slots(temper_channel::machine::Endpoint::WorkerLink, &TINY.sizes)
                .expect("record bound")
    );
    assert_eq!(world.link.state(), link::State::Open);
}
#[test]
fn startup_counts_all_tiny_sent_records_within_byte_credit() {
    let mut io = io_limits();
    io.sends = temper_channel::sizes::output_slots(temper_channel::machine::Endpoint::WorkerLink, &LIMITS.sizes)
        .expect("category slots");
    assert!(!LIMITS.fits_io(&io), "pending category count does not bound records already sent below");
    io.sends = temper_channel::sizes::stream_slots(temper_channel::machine::Endpoint::WorkerLink, &LIMITS.sizes)
        .expect("byte credit count");
    assert!(LIMITS.fits_io(&io));
}
