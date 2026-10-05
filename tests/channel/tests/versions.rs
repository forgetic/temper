//! Version negotiation and unknown-opcode framing through the fragmented world.
use skein_lib::Duration;
use temper_channel::{
    codec,
    machine::{Endpoint, Fault, Phase, Request},
    sizes,
    wire::{self, Message, v2},
};
use temper_channel_world::{Seen, World};

fn link(seed: u64, highest: u16, version: u16) -> World {
    let mut world = World::with_versions(Endpoint::Engine, seed, 1, highest);
    world.wire_version = version;
    world.append(&Message::Open {
        open: wire::Open {
            channel: wire::Channel::Link,
            lowest: 1,
            highest: version,
            name: Box::from(*b"worker"),
            secret: Box::from(*b"secret"),
        },
    });
    world.settle(500);
    assert_eq!(world.machine.phase(), Phase::Authorizing);
    world.request(Request::Accept { version });
    world.append(&Message::Terms {
        terms: sizes::terms_version(Endpoint::WorkerLink, &world.sizes, version).expect("version terms fit"),
    });
    if version == 1 {
        world.append(&Message::Hello { slots: 2, workstreams: Box::from([]), hosting: Box::from([]) });
    } else {
        world.append(&Message::HelloV2 {
            hello: v2::Hello {
                slots: 2,
                workstreams: Box::from([]),
                hosting: Box::from([]),
                graces: Duration::from_secs(5),
                push_deadline: Duration::from_secs(1),
            },
        });
    }
    world.settle(1000);
    assert_eq!(world.machine.phase(), Phase::Open);
    assert_eq!(world.machine.version(), version);
    world
}
fn unknown(kind: u16, length: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&kind.to_be_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.resize(usize::try_from(length).expect("small body") + 8, 41);
    bytes
}
fn statuses(world: &World, kind: u16) -> usize {
    world
        .seen
        .iter()
        .filter(|event| match event {
            Seen::Output(bytes) => {
                codec::decode_version(bytes, &world.sizes, world.wire_version) == Some(Message::Unsupported { kind })
            }
            Seen::Frame(_)
            | Seen::Ready(_)
            | Seen::Sent
            | Seen::Unsent
            | Seen::ReadEnded
            | Seen::Closed(_)
            | Seen::Finish => false,
        })
        .count()
}

#[test]
fn both_versions_keep_their_initial_layout_and_terms() {
    for version in [1, 2] {
        let world = link(7, 2, version);
        assert!(world.seen.iter().any(|event| matches!(event, Seen::Ready(found) if *found == version)));
        assert_eq!(world.machine.received_frames(), 3);
    }
}
#[test]
fn a_v1_peer_refuses_a_new_turn_and_keeps_frame_synchronization() {
    let mut world = link(31, 1, 1);
    let turn = Message::Turn {
        turn: v2::Turn { run: 1, attempt: 1, turn: 1, spent: 4, read: Some(2), body: Box::from(*b"turn") },
    };
    world.raw(&codec::encode_version(&turn, &world.sizes, 2).expect("new turn fits"));
    world.append(&Message::Relay { run: 1, attempt: 1, call: 9, body: Box::from(*b"next") });
    world.request(Request::Read);
    world.settle(2000);
    assert_eq!(statuses(&world, 264), 1);
    assert!(world.seen.iter().any(|event| matches!(event, Seen::Frame(Message::Relay { call: 9, .. }))));
    assert_eq!(world.machine.phase(), Phase::Open);
    assert_eq!(world.machine.received_frames(), 5);
}
#[test]
fn unknown_zero_and_chunked_bodies_are_skipped_then_valid_frames_dispatch() {
    for version in [1, 2] {
        let mut world = link(43, 2, version);
        world.raw(&unknown(0x109, 0));
        world.raw(&unknown(0x10a, 31));
        world.append(&Message::Relay { run: 1, attempt: 1, call: 11, body: Box::from(*b"next") });
        world.request(Request::Read);
        world.settle(3000);
        assert_eq!(statuses(&world, 0x109), 1);
        assert_eq!(statuses(&world, 0x10a), 1);
        assert!(world.seen.iter().any(|event| matches!(event, Seen::Frame(Message::Relay { call: 11, .. }))));
        assert_eq!(world.machine.phase(), Phase::Open);
    }
}
#[test]
fn bad_lengths_and_wrong_direction_still_close() {
    for version in [1, 2] {
        for kind in [0x109, 0x187] {
            let mut world = link(19, 2, version);
            let length = if kind == 0x109 {
                sizes::largest_body(&world.sizes, version).expect("body bound").checked_add(1).expect("small bound")
            } else {
                0
            };
            world.raw(&unknown(kind, length));
            world.request(Request::Read);
            world.settle(1000);
            assert_eq!(world.machine.phase(), Phase::Closed);
            assert!(world.seen.iter().any(|event| matches!(event, Seen::Closed(Fault::Framing))));
        }
    }
}
#[test]
fn v2_turn_dispatch_and_ack_busy_are_named() {
    let mut world = link(11, 2, 2);
    let turn = v2::Turn { run: 3, attempt: 2, turn: 1, spent: 17, read: None, body: Box::from(*b"turn") };
    world.append(&Message::Turn { turn: turn.clone() });
    world.request(Request::Read);
    world.settle(1000);
    assert!(
        world.seen.iter().any(|event| matches!(event, Seen::Frame(Message::Turn { turn: found }) if *found == turn))
    );
    for message in [
        Message::AcknowledgeTurn { turn: v2::TurnName { run: 3, attempt: 2, turn: 1 } },
        Message::TurnBusy { turn: v2::TurnName { run: 3, attempt: 2, turn: 2 } },
    ] {
        world.request(Request::Send(message.clone()));
        world.settle(1000);
        assert!(world.seen.iter().any(|event| match event {
            Seen::Output(bytes) => codec::decode_version(bytes, &world.sizes, 2) == Some(message.clone()),
            Seen::Frame(_)
            | Seen::Ready(_)
            | Seen::Sent
            | Seen::Unsent
            | Seen::ReadEnded
            | Seen::Closed(_)
            | Seen::Finish => false,
        }));
    }
}
#[test]
fn send_never_uses_a_v2_shape_on_a_v1_channel() {
    let mut world = link(59, 2, 1);
    world.request(Request::Send(Message::AcknowledgeTurn { turn: v2::TurnName { run: 1, attempt: 1, turn: 1 } }));
    assert!(world.seen.iter().any(|event| matches!(event, Seen::Unsent)));
    assert_eq!(world.machine.phase(), Phase::Open);
    assert_eq!(statuses(&world, 391), 0);
}
#[test]
fn agent_picks_highest_supported_and_exchanges_start_and_finish() {
    let mut world = World::with_versions(Endpoint::Agent, 23, 1, 2);
    world.append(&Message::Open {
        open: wire::Open {
            channel: wire::Channel::Agent,
            lowest: 1,
            highest: 2,
            name: Box::from([]),
            secret: Box::from([]),
        },
    });
    world.append(&Message::Terms {
        terms: sizes::terms_version(Endpoint::WorkerAgent, &world.sizes, 2).expect("terms fit"),
    });
    world.append(&Message::AgentStartV2 {
        start: v2::AgentStart {
            charter: Box::from(*b"charter"),
            transcript: Some(Box::from(*b"transcript")),
            repositories: Box::from([v2::AgentRepository {
                name: Box::from(*b"repo"),
                writable: true,
                conflicts: Box::from([Box::from(*b"file")]),
            }]),
            endpoints: Box::from([]),
            grants: Box::from([]),
        },
    });
    world.settle(1000);
    assert_eq!(world.machine.phase(), Phase::Open);
    assert_eq!(world.machine.version(), 2);
    world.request(Request::Send(Message::FinishV2 {
        finish: v2::Finished { turns: 4, spent: 71, finish: v2::Finish::Parked },
    }));
    world.settle(1000);
    assert_eq!(world.machine.phase(), Phase::Closing);
}
#[test]
fn acceptance_must_fit_both_configured_and_offered_ranges() {
    let mut world = World::with_versions(Endpoint::WorkerLink, 67, 1, 2);
    world.wire_version = 1;
    world.request(Request::Send(Message::Open {
        open: wire::Open {
            channel: wire::Channel::Link,
            lowest: 1,
            highest: 1,
            name: Box::from(*b"worker"),
            secret: Box::from(*b"secret"),
        },
    }));
    world.append(&Message::Accept { version: 2 });
    world.settle(1000);
    assert_eq!(world.machine.phase(), Phase::Closed);
    assert!(world.seen.iter().any(|event| matches!(event, Seen::Closed(Fault::Version))));
}

#[test]
fn unsupported_flood_is_bounded_and_eventually_resumes() {
    let mut world = link(83, 2, 1);
    for _ in 0..128 {
        world.raw(&unknown(0x10b, 17));
    }
    world.append(&Message::Relay { run: 1, attempt: 1, call: 12, body: Box::from(*b"next") });
    world.request(Request::Read);
    world.settle(20_000);
    assert_eq!(statuses(&world, 0x10b), 128);
    assert!(world.seen.iter().any(|event| matches!(event, Seen::Frame(Message::Relay { call: 12, .. }))));
    assert_eq!(world.machine.phase(), Phase::Open);
    assert!(
        world.machine.queued_bytes() <= sizes::output_cap_version(Endpoint::Engine, &world.sizes, 2).expect("cap fits")
    );
}
#[test]
fn new_version_world_trace_replays() {
    for version in [1, 2] {
        let first = link(97, 2, version);
        let second = link(97, 2, version);
        assert_eq!(first.seen, second.seen);
    }
}

#[test]
fn a_configured_range_bound_includes_both_versions_before_negotiation() {
    let sizes = temper_channel::Sizes { snapshot: 1_048_576, transcript: 16, ..temper_channel::Sizes::STARTING };
    let limits = temper_channel::Limits::STARTING;
    for endpoint in [Endpoint::Engine, Endpoint::WorkerLink, Endpoint::WorkerAgent, Endpoint::Agent] {
        let both = sizes::worst_case_versions(endpoint, &limits, &sizes, 1, 2).expect("range bound");
        for version in [1, 2] {
            assert!(both >= sizes::worst_case_version(endpoint, &limits, &sizes, version).expect("version bound"));
        }
    }
    assert!(sizes::worst_case_versions(Endpoint::Engine, &limits, &sizes, 2, 1).is_none());
    assert!(sizes::worst_case_versions(Endpoint::Engine, &limits, &sizes, 0, 2).is_none());
}
