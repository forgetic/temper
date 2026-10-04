use skein_lib::stream;
use temper_channel::{
    codec,
    machine::{Endpoint, Fault, Phase, Request},
    wire::{Finish, Message, RunFailure},
};
use temper_channel_world::{Seen, World, replay};

#[test]
fn fragmented_link_replays_and_backpressure_stops_reading() {
    assert_eq!(replay(71), replay(71));
    let mut world = World::new(Endpoint::Agent, 9);
    world.open_agent();
    world.append(&Message::AgentEvent { event: 1, body: Box::from(*b"one") });
    world.append(&Message::AgentEvent { event: 2, body: Box::from(*b"two") });
    let before = world.machine.received_frames();
    world.settle(500);
    assert_eq!(world.machine.received_frames(), before, "the user has not demanded another message");
    world.request(Request::Read);
    world.settle(500);
    assert_eq!(world.machine.received_frames(), before + 1);
    world.settle(500);
    assert_eq!(world.machine.received_frames(), before + 1);
}
#[test]
fn agent_read_eof_keeps_output_for_finish() {
    let mut world = World::new(Endpoint::Agent, 3);
    world.open_agent();
    world.end();
    assert_eq!(world.machine.phase(), Phase::Open);
    assert_eq!(world.seen.iter().filter(|e| matches!(e, Seen::ReadEnded)).count(), 1);
    world.request(Request::Send(Message::Finish { finish: Finish::Failed { failure: RunFailure::Cancelled } }));
    world.settle(1000);
    assert_eq!(world.machine.phase(), Phase::Closing);
    assert!(world.seen.iter().any(|e| matches!(e, Seen::Finish)));
    assert!(world.seen.iter().any(|e| match e {
        Seen::Output(bytes) => matches!(codec::decode(bytes, &world.sizes), Some(Message::Finish { .. })),
        Seen::Frame(_)
        | Seen::Ready(_)
        | Seen::Sent
        | Seen::Unsent
        | Seen::ReadEnded
        | Seen::Closed(_)
        | Seen::Finish => false,
    }));
    world.request(Request::Send(Message::LongDone));
    assert!(world.seen.iter().any(|e| matches!(e, Seen::Unsent)));
}
#[test]
fn peer_wrong_direction_reserved_and_oversized_close() {
    for header in [[2, 1, 0, 0, 0, 0, 0, 0], [0, 1, 0, 1, 0, 0, 0, 0], [0, 1, 0, 0, 255, 255, 255, 255]] {
        let mut world = World::new(Endpoint::Engine, 4);
        world.raw(&header);
        world.settle(100);
        assert_eq!(world.machine.phase(), Phase::Closed);
        assert!(world.seen.iter().any(|e| matches!(e, Seen::Closed(Fault::Framing))));
    }
}
#[test]
fn end_and_fail_at_each_partial_frame_offset() {
    let mut sample = World::new(Endpoint::Engine, 1);
    sample.append(&Message::Open {
        open: temper_channel::wire::Open {
            channel: temper_channel::wire::Channel::Link,
            lowest: 1,
            highest: 1,
            name: Box::from(*b"worker"),
            secret: Box::from(*b"secret"),
        },
    });
    let bytes = codec::encode(
        &Message::Open {
            open: temper_channel::wire::Open {
                channel: temper_channel::wire::Channel::Link,
                lowest: 1,
                highest: 1,
                name: Box::from(*b"worker"),
                secret: Box::from(*b"secret"),
            },
        },
        &sample.sizes,
    )
    .expect("frame fits");
    for cut in 0..bytes.len() {
        for failed in [false, true] {
            let mut world = World::new(Endpoint::Engine, u64::try_from(cut).expect("short frame"));
            world.raw(&bytes[..cut]);
            world.settle(100);
            if failed {
                world.input(stream::Up::Failed(stream::Fault::Reset));
            } else {
                world.end();
            }
            assert_eq!(world.machine.phase(), Phase::Closed);
            assert_eq!(world.seen.iter().filter(|e| matches!(e, Seen::Closed(_))).count(), 1);
        }
    }
}
#[test]
fn send_while_read_is_outstanding_uses_held_credit() {
    let mut world = World::new(Endpoint::Agent, 5);
    world.open_agent();
    world.request(Request::Read);
    world.settle(100);
    assert!(world.demand.is_some());
    let before = world.seen.len();
    world.request(Request::Send(Message::Long { span: skein_lib::Duration::from_secs(1) }));
    assert!(world.seen[before..].iter().any(|e| matches!(e, Seen::Output(_))));
    assert_eq!(world.machine.phase(), Phase::Open);
}
#[test]
fn golden_fixtures_decode_and_roundtrip() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("transcripts");
    for entry in std::fs::read_dir(directory).expect("fixtures exist") {
        let bytes = std::fs::read(entry.expect("fixture entry").path()).expect("fixture readable");
        let decoded = codec::decode(&bytes, &temper_channel::Sizes::STARTING).expect("golden decodes");
        assert_eq!(codec::encode(&decoded, &temper_channel::Sizes::STARTING).expect("golden encodes").as_ref(), bytes);
    }
}

#[test]
fn version_range_and_agent_identity_are_checked_before_terms() {
    for (lowest, highest, name) in [(2, 7, b"".as_slice()), (7, 1, b"".as_slice()), (1, 1, b"unexpected".as_slice())] {
        let mut world = World::new(Endpoint::Agent, 7);
        world.append(&Message::Open {
            open: temper_channel::wire::Open {
                channel: temper_channel::wire::Channel::Agent,
                lowest,
                highest,
                name: name.into(),
                secret: Box::from([]),
            },
        });
        world.settle(200);
        assert_eq!(world.machine.phase(), Phase::Closed);
        assert!(!world.seen.iter().any(|e| matches!(e, Seen::Ready(_))));
    }
}
#[test]
fn pings_are_validated_progress_without_domain_messages() {
    let mut world = World::new(Endpoint::Engine, 11);
    world.append(&Message::Open {
        open: temper_channel::wire::Open {
            channel: temper_channel::wire::Channel::Link,
            lowest: 1,
            highest: 1,
            name: Box::from([]),
            secret: Box::from([]),
        },
    });
    world.settle(500);
    world.request(Request::Accept { version: 1 });
    world.append(&Message::Terms {
        terms: temper_channel::sizes::terms(Endpoint::WorkerLink, &world.sizes).expect("terms fit"),
    });
    world.append(&Message::Ping);
    world.append(&Message::Hello { slots: 1, workstreams: Box::from([]), hosting: Box::from([]) });
    world.settle(500);
    assert_eq!(world.machine.phase(), Phase::Open);
    assert_eq!(world.machine.received_frames(), 4);
    assert!(!world.seen.iter().any(|e| matches!(e, Seen::Frame(Message::Ping))));
}

#[test]
fn a_queued_room_grant_debits_sends_that_cross_its_delivery() {
    let mut world = World::new(Endpoint::Agent, 18);
    world.open_agent();
    world.request(Request::Send(Message::LongDone));
    world.hold_room();
    let cap = world.credit();
    world.request(Request::Send(Message::Long { span: skein_lib::Duration::from_secs(1) }));
    assert_eq!(world.credit(), cap - 16);
    world.deliver_room();
    assert_eq!(world.machine.room(), cap - 16, "queued Room must not restore consumed credit");
    assert_eq!(world.machine.phase(), Phase::Open);
    world.settle(100);
    assert_eq!(world.machine.room(), cap, "a fresh grant recovers conservative credit");
}

#[test]
fn a_terminal_ignores_withdrawn_demand_answers_and_stale_refusals() {
    for room in [false, true] {
        let mut world = World::new(Endpoint::Agent, 23);
        world.open_agent();
        if room {
            world.request(Request::Send(Message::LongDone));
            world.hold_room();
        } else {
            world.request(Request::Read);
            assert_eq!(world.demand.map(|(_, room)| room), Some(0));
        }
        world.request(Request::Send(Message::Finish { finish: Finish::Failed { failure: RunFailure::Cancelled } }));
        assert_eq!(world.machine.phase(), Phase::Closing);
        let finished = world.seen.len();
        if room {
            world.deliver_room();
        } else {
            world.input(stream::Up::Bytes(Box::new([0xff; 8])));
        }
        world.request(Request::Refuse(temper_channel::wire::Refuse { reason: 6, text: Box::new([]) }));
        assert_eq!(world.machine.phase(), Phase::Closing);
        assert_eq!(world.seen.len(), finished, "nothing is sent after output Finish");
    }
}

#[test]
fn eof_withdraws_read_only_demand_and_preserves_new_room_after_late_bytes() {
    let mut world = World::new(Endpoint::Agent, 24);
    world.open_agent();
    world.request(Request::Read);
    assert_eq!(world.demand.map(|(_, room)| room), Some(0));
    world.end();
    assert!(world.demand.is_none(), "the lower read demand was explicitly withdrawn");
    world.request(Request::Send(Message::LongDone));
    assert_eq!(world.demand.map(|(read, _)| read), Some(stream::Read::Nothing));
    world.input(stream::Up::Bytes(Box::new([0xff; 8])));
    assert_eq!(world.machine.phase(), Phase::Open);
    assert!(world.demand.is_some(), "crossed old Bytes cannot answer the new room demand");
    world.hold_room();
    world.deliver_room();
    world.request(Request::Send(Message::Finish { finish: Finish::Failed { failure: RunFailure::Cancelled } }));
    assert_eq!(world.machine.phase(), Phase::Closing);
}

#[test]
fn a_terminal_waiting_for_initial_room_ignores_crossed_bytes_until_flushed() {
    let mut world = World::new(Endpoint::Agent, 27);
    world.tick();
    assert_eq!(world.machine.room(), 0);
    world.request(Request::Refuse(temper_channel::wire::Refuse { reason: 6, text: Box::new([]) }));
    assert_eq!(world.machine.phase(), Phase::Finished);
    assert!(!world.seen.iter().any(|event| matches!(event, Seen::Output(_) | Seen::Finish)));
    world.input(stream::Up::Bytes(Box::new([0xff; 8])));
    assert_eq!(world.machine.phase(), Phase::Finished);
    assert_eq!(world.demand.map(|(read, _)| read), Some(stream::Read::Nothing));
    world.hold_room();
    world.deliver_room();
    assert_eq!(world.machine.phase(), Phase::Closing);
    let [Seen::Output(bytes), Seen::Finish] = &world.seen[..] else {
        panic!("refusal then one output Finish: {:?}", world.seen);
    };
    assert!(matches!(codec::decode(bytes, &world.sizes), Some(Message::Refuse { .. })));
}

#[test]
fn worker_reads_stdout_through_eof_after_incoming_finish() {
    let mut world = World::new(Endpoint::WorkerAgent, 29);
    world.request(Request::Send(Message::Open {
        open: temper_channel::wire::Open {
            channel: temper_channel::wire::Channel::Agent,
            lowest: 1,
            highest: 1,
            name: Box::new([]),
            secret: Box::new([]),
        },
    }));
    world.append(&Message::Accept { version: 1 });
    world.append(&Message::Terms {
        terms: temper_channel::sizes::terms(Endpoint::Agent, &world.sizes).expect("agent terms fit"),
    });
    world.settle(1000);
    world.request(Request::Send(Message::AgentStart {
        charter: Box::new([]),
        snapshot: None,
        repositories: Box::new([]),
        endpoints: Box::new([]),
        grants: Box::new([]),
    }));
    world.append(&Message::Finish { finish: Finish::Failed { failure: RunFailure::Cancelled } });
    world.request(Request::Read);
    world.settle(1000);
    assert_eq!(world.machine.phase(), Phase::Open);
    assert!(world.seen.iter().any(|event| matches!(event, Seen::Frame(Message::Finish { .. }))));
    world.append(&Message::LongDone);
    world.request(Request::Read);
    world.settle(1000);
    assert!(
        world.seen.iter().any(|event| matches!(event, Seen::Frame(Message::LongDone))),
        "the worker domain receives and faults a late agent frame"
    );
    world.end();
    assert_eq!(world.machine.phase(), Phase::Closed);
}
