use skein_io as io;
use skein_lib::stream;
use temper_channel::wire;
use temper_engine_protocol::{
    connection::{Phase, Transport},
    listener::{self, Event, Notice, Security},
    names,
};
use temper_engine_protocol_world::link::{LIMITS, SIZES, World};
use temper_legacy_engine_domain as engine;

#[test]
fn names_and_secrets_are_authenticated_before_any_domain_hello() {
    for (name, secret, valid) in [
        (b"alpha".as_slice(), b"one".as_slice(), true),
        (b"alpha", b"owe", false),
        (b"unknown", b"one", false),
        (b"alpha", b"one\0", false),
    ] {
        let mut world = World::new(Transport::Loopback);
        let (socket, owner) = world.open(name, secret);
        assert_eq!(world.phase(owner), Some(if valid { Phase::Open } else { Phase::Closing }));
        let hellos =
            world.notices.iter().filter(|notice| matches!(notice, Notice::Domain(engine::Event::Hello { .. }))).count();
        assert_eq!(hellos, usize::from(valid));
        if !valid {
            assert!(
                world.peers[&socket]
                    .frames
                    .iter()
                    .any(|frame| matches!(frame, wire::Message::Refuse { refuse } if refuse.reason == 2))
            );
            world.closed(socket);
            assert!(!world.notices.iter().any(|notice| matches!(notice, Notice::Domain(engine::Event::Lost { .. }))));
        }
    }
}

#[test]
fn replacement_waits_for_actual_closed_then_loses_the_old_channel_before_new_hello() {
    let mut world = World::limited(Transport::Loopback, temper_engine_protocol::Limits { connections: 2, ..LIMITS });
    let (old_socket, old) = world.open(b"alpha", b"one");
    world.notices.clear();
    let (new_socket, new) = world.open(b"alpha", b"one");
    assert!(world.notices.is_empty(), "old link is not Lost before actual Closed");
    assert_eq!(world.listener.connections(), 2);
    assert_eq!(world.phase(old), Some(Phase::Closing));
    assert_eq!(world.phase(new), Some(Phase::Pending));
    assert_eq!(world.listener.channel(0), Some(old));
    assert!(!world.peers[&new_socket].frames.iter().any(|frame| matches!(frame, wire::Message::Accept { .. })));
    let late = temper_channel::codec::encode(&wire::Message::Told { run: 1, attempt: 1, fact: Box::new([]) }, &SIZES)
        .expect("late fact frame fits");
    world.input(Event::Io(io::Event::Stream { owner: old, up: stream::Up::Bytes(late) }));
    assert!(world.notices.is_empty(), "old input is fenced during replacement close");
    let (refused, owner) = world.accept();
    assert!(owner.is_none());
    assert!(world.peers[&refused].rejected);
    world.closed(old_socket);
    world.settle();
    let notices: Vec<_> = world
        .notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::Domain(engine::Event::Lost { channel }) => Some((0, *channel)),
            Notice::Domain(engine::Event::Hello { channel, .. }) => Some((1, *channel)),
            Notice::Domain(_) | Notice::Listening { .. } | Notice::Failed { .. } | Notice::Closed => None,
        })
        .collect();
    assert_eq!(notices, [(0, old), (1, new)]);
    assert_eq!(world.listener.connections(), 1);
    assert_eq!(world.listener.channel(0), Some(new));
    let (_, admitted) = world.accept();
    assert!(admitted.is_some());
    world.input(Event::Io(io::Event::Stream { owner: old, up: stream::Up::Failed(stream::Fault::Reset) }));
    world.input(Event::Io(io::Event::Closed { owner: old }));
    assert_eq!(world.phase(new), Some(Phase::Open));
    assert_eq!(
        world
            .notices
            .iter()
            .filter(|notice| matches!(notice, Notice::Domain(engine::Event::Lost { channel }) if *channel == old))
            .count(),
        1
    );
}

#[test]
fn newest_authenticated_replacement_wins_the_bounded_pending_slot() {
    let mut world = World::new(Transport::Loopback);
    let (old_socket, old) = world.open(b"alpha", b"one");
    let (pending_socket, pending) = world.open(b"alpha", b"one");
    let (_, newest) = world.open(b"alpha", b"one");
    assert_eq!(world.phase(old), Some(Phase::Closing));
    assert_eq!(world.phase(pending), Some(Phase::Closing));
    assert_eq!(world.phase(newest), Some(Phase::Pending));
    assert!(
        world.peers[&pending_socket]
            .frames
            .iter()
            .any(|frame| matches!(frame, wire::Message::Refuse { refuse } if refuse.reason == 5))
    );
    world.closed(old_socket);
    world.settle();
    assert_eq!(world.phase(newest), Some(Phase::Open));
    world.closed(pending_socket);
    assert_eq!(world.listener.channel(0), Some(newest));
}

#[test]
fn waiting_replacement_keeps_its_original_handshake_deadline() {
    let mut world = World::new(Transport::Loopback);
    let (old_socket, _) = world.open(b"alpha", b"one");
    let (pending_socket, pending) = world.open(b"alpha", b"one");
    world.seconds(5);
    world.fire();
    assert_eq!(world.phase(pending), Some(Phase::Closing));
    world.closed(old_socket);
    world.settle();
    assert_eq!(world.listener.channel(0), None);
    assert!(!world.peers[&pending_socket].frames.iter().any(|frame| matches!(frame, wire::Message::Accept { .. })));
}

#[test]
fn handshake_and_first_hello_deadlines_never_announce_unopened_channels() {
    for terms in [false, true] {
        let mut world = World::new(Transport::Loopback);
        let (socket, owner) = world.accept();
        let owner = owner.expect("admitted");
        world.opening(socket, b"alpha", b"one", terms, false);
        world.settle();
        world.seconds(5);
        world.fire();
        assert_eq!(world.phase(owner), Some(Phase::Closing));
        world.closed(socket);
        assert!(!world.notices.iter().any(|notice| matches!(notice, Notice::Domain(_))));
    }
}

#[test]
fn ping_input_resets_silence_but_an_unanswered_link_eventually_closes() {
    let mut world = World::new(Transport::Loopback);
    let (socket, owner) = world.open(b"alpha", b"one");
    world.seconds(3);
    world.fire();
    assert!(world.peers[&socket].frames.iter().any(|frame| matches!(frame, wire::Message::Ping)));
    world.seconds(7);
    world.append(socket, wire::Message::Ping);
    world.settle();
    world.seconds(9);
    world.fire();
    assert_eq!(world.phase(owner), Some(Phase::Open));
    world.seconds(16);
    world.fire();
    assert_eq!(world.phase(owner), Some(Phase::Closing));
    assert!(!world.notices.iter().any(|notice| matches!(notice, Notice::Domain(engine::Event::Lost { .. }))));
    world.closed(socket);
    assert!(
        world
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::Domain(engine::Event::Lost { channel }) if *channel == owner))
    );
}

#[test]
fn blocked_output_closes_on_stall_while_inbound_pings_keep_arriving() {
    let mut world = World::new(Transport::Loopback);
    let (socket, owner) = world.open(b"alpha", b"one");
    world.peers.get_mut(&socket).expect("peer").blocked = true;
    world.down(
        engine::Request::Cancel { channel: owner, item: engine::Item { repository: 0, number: 1 }, attempt: 1 },
        &[],
    );
    for seconds in [1, 3, 5] {
        world.seconds(seconds);
        world.append(socket, wire::Message::Ping);
        world.settle();
        world.fire();
    }
    world.seconds(6);
    world.fire();
    assert_eq!(world.phase(owner), Some(Phase::Closing));
    world.seconds(12);
    world.fire();
    assert!(world.peers[&socket].aborted);
    world.seconds(20);
    world.fire();
    assert_eq!(world.listener.connections(), 1, "io Closed alone releases the binding");
}

#[test]
fn malformed_relay_is_answered_directly_and_snapshot_bytes_remain_opaque() {
    let mut world = World::new(Transport::Loopback);
    let (socket, owner) = world.open(b"alpha", b"one");
    let item = engine::Item { repository: 0, number: 42 };
    let run = names::run(item).expect("fits").raw();
    let attempt = names::attempt(item, 3).expect("fits").raw();
    world.append(socket, wire::Message::Relay { run, attempt, call: 7, body: Box::new([]) });
    world.settle();
    let reply = world.peers[&socket]
        .frames
        .iter()
        .find_map(|frame| {
            if let wire::Message::Relayed { run: a, attempt: b, call, answer } = frame
                && (*a, *b, *call) == (run, attempt, 7)
            {
                Some(answer)
            } else {
                None
            }
        })
        .expect("invalid relay answered");
    assert_eq!(
        temper_engine_protocol::payload::decode_served(reply, &SIZES),
        Some(engine::Served::Unserved(engine::Unserved::Invalid))
    );
    assert_eq!(world.phase(owner), Some(Phase::Open));
    world.append(
        socket,
        wire::Message::Answer {
            run,
            attempt,
            answer: wire::LinkAnswer::Parked {
                snapshot: Some([0, 99, 4].into()),
                work: wire::Work { landed: Box::new([]), saved: None },
            },
        },
    );
    world.settle();
    assert!(world.notices.iter().any(|notice| matches!(notice, Notice::Domain(engine::Event::Answer { answer: engine::Answer::Parked { snapshot: Some(bytes), .. }, .. }) if **bytes == [0,99,4])));
}

#[test]
fn secured_connections_require_an_explicit_tls_completion_and_raw_bytes_stay_below() {
    let mut world = World::new(Transport::Secured);
    let (socket, owner) = world.accept();
    let owner = owner.expect("admitted");
    world.input(Event::Io(io::Event::Stream { owner, up: stream::Up::Bytes(b"raw ciphertext".as_slice().into()) }));
    assert_eq!(world.phase(owner), Some(Phase::Securing));
    assert!(world.security.iter().any(|event| matches!(event, Security::Input { owner: of, .. } if *of == owner)));
    world.input(Event::Secured { owner });
    world.opening(socket, b"alpha", b"one", true, true);
    world.settle();
    assert_eq!(world.phase(owner), Some(Phase::Open));
    world.input(Event::SecurityFailed { owner });
    assert_eq!(world.phase(owner), Some(Phase::Closing));
    world.closed(socket);
    assert_eq!(world.listener.connections(), 0);
}

#[test]
fn shutdown_is_bounded_and_terminal_only_after_all_lower_entities_close() {
    let mut world = World::new(Transport::Loopback);
    let (socket, _) = world.open(b"alpha", b"one");
    world.shutdown();
    world.input(Event::Io(io::Event::Closed { owner: listener::OWNER }));
    world.settle();
    assert!(!world.listener.is_closed());
    world.closed(socket);
    world.settle();
    assert!(world.listener.is_closed());
    assert_eq!(world.notices.iter().filter(|notice| matches!(notice, Notice::Closed)).count(), 1);
}

#[test]
fn incompatible_secret_caps_and_nonloopback_plaintext_are_refused_at_the_entrance() {
    use temper_engine_protocol::{Limits, listener::Listener};
    use temper_engine_protocol_world::link::{io_limits, repositories, workers};
    let sizes = temper_channel::Sizes { secret_bytes: 32, ..SIZES };
    let io = io_limits(&LIMITS, &SIZES);
    assert!(Listener::new(workers(), repositories(), Transport::Loopback, &LIMITS, &sizes, &io).is_none());
    assert!(temper_engine_protocol::worst_case(&Limits { repositories: 257, ..LIMITS }, &SIZES).is_none());
    let mut listener =
        Listener::new(workers(), repositories(), Transport::Loopback, &LIMITS, &SIZES, &io).expect("fits");
    let mut effects = skein_lib::Queue::with_capacity(listener::MAX_OUT);
    assert!(!listener::start(&mut listener, "0.0.0.0:42".parse().expect("address"), &mut effects));
    assert!(effects.is_empty());
}

#[test]
fn startup_rejects_io_that_cannot_hold_tiny_frames_under_the_byte_grant() {
    use temper_engine_protocol::listener::Listener;
    use temper_engine_protocol_world::link::{io_limits, repositories, workers};
    let cap = temper_channel::sizes::output_cap(temper_channel::machine::Endpoint::Engine, &SIZES).expect("cap");
    let mut io = io_limits(&LIMITS, &SIZES);
    // One Send is in flight, the rest need queued record slots. Both byte
    // capacity and category-derived slots fit in the rejected configuration.
    io.sends = cap / 8 - 2;
    assert!(
        io.sends
            >= temper_channel::sizes::output_slots(temper_channel::machine::Endpoint::Engine, &SIZES)
                .expect("category slots")
    );
    assert!(!LIMITS.fits_io(&SIZES, &io));
    assert!(Listener::new(workers(), repositories(), Transport::Loopback, &LIMITS, &SIZES, &io).is_none());
    io.sends += 1;
    assert!(LIMITS.fits_io(&SIZES, &io));
    assert!(Listener::new(workers(), repositories(), Transport::Loopback, &LIMITS, &SIZES, &io).is_some());
    for short in [io::Limits { intake: LIMITS.channel.chunk.max(8) - 1, ..io }, io::Limits { output: cap - 1, ..io }] {
        assert!(!LIMITS.fits_io(&SIZES, &short));
        assert!(Listener::new(workers(), repositories(), Transport::Loopback, &LIMITS, &SIZES, &short).is_none());
    }
}

#[test]
fn seeded_byte_lifecycles_replay_with_distinct_races() {
    let trace = temper_world::assert_replays(9, 10, |seed| temper_engine_protocol_world::link::random(seed, 32));
    assert!(trace.len() > 32);
}
