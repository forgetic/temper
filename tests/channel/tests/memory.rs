//! Measure demanded bodies and decoded array storage with maximum byte fields.
use skein_lib::{Queue, stream};
use temper_channel::{
    Limits, Sizes, codec,
    machine::{self, Endpoint, Machine},
    sizes,
    wire::{Channel, Message, Open},
};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
#[test]
fn demanded_body_and_decode_stay_within_exact_bound() {
    let limits = Limits { chunk: 1, ..Limits::STARTING };
    let sizes = Sizes { name_bytes: 256, ..Sizes::STARTING };
    let input = Message::Open {
        open: Open {
            channel: Channel::Link,
            lowest: 1,
            highest: 1,
            name: vec![1; 64].into_boxed_slice(),
            secret: vec![2; 64].into_boxed_slice(),
        },
    };
    let frame = codec::encode(&input, &sizes).expect("maximum frozen frame fits");
    let mut upper = Queue::with_capacity(machine::MAX_UP);
    let mut lower = Queue::with_capacity(machine::MAX_DOWN);
    let bound = sizes::worst_case(Endpoint::Engine, &limits, &sizes).expect("bound fits");
    let meter = Meter::new();
    let mut machine = Machine::new(Endpoint::Engine, &limits, &sizes).expect("machine fits");
    meter.start();
    machine::poll(&mut machine, &limits, &sizes, &mut upper, &mut lower);
    let step = meter.end();
    while let Some(request) = lower.pop() {
        drop(request);
    }
    meter.check(step, bound, "initial demand");
    let mut at = 0;
    while at < frame.len() {
        let count = if at == 0 { 8 } else { 1 };
        let bytes = frame[at..at + count].into();
        at += count;
        meter.start();
        machine::up(&mut machine, &limits, &sizes, stream::Up::Bytes(bytes), &mut upper, &mut lower);
        let step = meter.end();
        while let Some(request) = lower.pop() {
            drop(request);
        }
        while let Some(event) = upper.pop() {
            drop(event);
        }
        meter.check(step, bound, "header, chunk or decoded open");
    }
    assert_eq!(machine.received_frames(), 1);
    assert!(meter.held() <= bound);
    drop(machine);
    assert_eq!(meter.held(), 0);
}

#[test]
#[expect(clippy::too_many_lines, reason = "complete maximum Start fixture and measured stream driver")]
fn maximum_start_arrays_and_fields_stay_within_bound() {
    use skein_lib::Duration;
    use temper_channel::wire::{Address, AgentRepository, EndpointDescriptor, Grant, Provider};
    let limits = Limits { chunk: 7, ..Limits::STARTING };
    let sizes = Sizes {
        charter: 512,
        snapshot: 256,
        name_bytes: 256,
        token_bytes: 128,
        repositories: 2,
        endpoints: 2,
        grants: 2,
        agent_outbox: 2,
        ..Sizes::STARTING
    };
    let bytes = |count: u32| vec![3; usize::try_from(count).expect("small fixture")].into_boxed_slice();
    let repositories = (0..sizes.repositories)
        .map(|_| AgentRepository { name: bytes(sizes.name_bytes), writable: true })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let endpoints = (0..sizes.endpoints)
        .map(|endpoint| EndpointDescriptor {
            endpoint,
            provider: Provider::OpenAi,
            host: bytes(sizes.name_bytes),
            address: Address::V6 { bytes: [7; 16] },
            port: 443,
            path: bytes(sizes.name_bytes),
            account: endpoint,
            effort: bytes(sizes.name_bytes),
            thinking: Some(u32::MAX),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let grants = (0..sizes.grants)
        .map(|account| Grant {
            account,
            generation: u64::MAX,
            valid: Duration::from_nanos(u64::MAX),
            token: bytes(sizes.token_bytes),
            account_id: bytes(sizes.token_bytes),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let messages = [
        Message::Open {
            open: Open { channel: Channel::Agent, lowest: 1, highest: 1, name: Box::from([]), secret: Box::from([]) },
        },
        Message::Terms { terms: sizes::terms(Endpoint::WorkerAgent, &sizes).expect("terms fit") },
        Message::AgentStart {
            charter: bytes(sizes.charter),
            snapshot: Some(bytes(sizes.snapshot)),
            repositories,
            endpoints,
            grants,
        },
    ];
    let frames =
        messages.iter().map(|message| codec::encode(message, &sizes).expect("maximum body fits")).collect::<Vec<_>>();
    drop(messages);
    let upper = Queue::with_capacity(machine::MAX_UP);
    let lower = Queue::with_capacity(machine::MAX_DOWN);
    let bound = sizes::worst_case(Endpoint::Agent, &limits, &sizes).expect("bound fits");
    let meter = Meter::new();
    let mut machine = Machine::new(Endpoint::Agent, &limits, &sizes).expect("machine fits");
    let mut upper = upper;
    let mut lower = lower;
    machine::poll(&mut machine, &limits, &sizes, &mut upper, &mut lower);
    let mut demand = None;
    while let Some(request) = lower.pop() {
        if let stream::Down::Demand { read, room } = request {
            demand = Some((read, room));
        }
    }
    for frame in &frames {
        let mut at = 0;
        for _ in 0..4096 {
            if at == frame.len() {
                break;
            }
            let (read, room) = demand.take().expect("the next read or credit was demanded");
            let input = if room > 0 {
                stream::Up::Room
            } else {
                let stream::Read::Fill(count) = read else {
                    unreachable!("the sized machine only fills");
                };
                let count = usize::try_from(count).expect("small chunk");
                let input = stream::Up::Bytes(frame[at..at + count].into());
                at += count;
                input
            };
            meter.start();
            machine::up(&mut machine, &limits, &sizes, input, &mut upper, &mut lower);
            let measured = meter.end();
            while let Some(request) = lower.pop() {
                if let stream::Down::Demand { read, room } = request {
                    demand = Some((read, room));
                }
            }
            while let Some(event) = upper.pop() {
                drop(event);
            }
            meter.check(measured, bound, "maximum Start repositories, endpoints and grants");
        }
        assert_eq!(at, frame.len());
    }
    assert_eq!(machine.received_frames(), 3);
    drop(machine);
    assert_eq!(meter.held(), 0);
}

#[test]
#[expect(clippy::too_many_lines, reason = "complete maximum Start fixture and measured stream driver")]
fn maximum_v2_start_arrays_conflicts_and_transcript_stay_within_bound() {
    use skein_lib::Duration;
    use temper_channel::wire::v2::{AgentRepository, AgentStart};
    use temper_channel::wire::{Address, EndpointDescriptor, Grant, Provider};
    let limits = Limits { chunk: 7, ..Limits::STARTING };
    let sizes = Sizes {
        charter: 512,
        snapshot: 256,
        transcript: 256,
        conflicts: 2,
        name_bytes: 256,
        token_bytes: 128,
        repositories: 2,
        endpoints: 2,
        grants: 2,
        agent_outbox: 2,
        ..Sizes::STARTING
    };
    let bytes = |count: u32| vec![3; usize::try_from(count).expect("small fixture")].into_boxed_slice();
    let repositories = (0..sizes.repositories)
        .map(|_| AgentRepository {
            name: bytes(sizes.name_bytes),
            writable: true,
            conflicts: (0..sizes.conflicts).map(|_| bytes(sizes.name_bytes)).collect::<Vec<_>>().into_boxed_slice(),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let endpoints = (0..sizes.endpoints)
        .map(|endpoint| EndpointDescriptor {
            endpoint,
            provider: Provider::OpenAi,
            host: bytes(sizes.name_bytes),
            address: Address::V6 { bytes: [7; 16] },
            port: 443,
            path: bytes(sizes.name_bytes),
            account: endpoint,
            effort: bytes(sizes.name_bytes),
            thinking: Some(u32::MAX),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let grants = (0..sizes.grants)
        .map(|account| Grant {
            account,
            generation: u64::MAX,
            valid: Duration::from_nanos(u64::MAX),
            token: bytes(sizes.token_bytes),
            account_id: bytes(sizes.token_bytes),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let messages = [
        Message::Open {
            open: Open { channel: Channel::Agent, lowest: 1, highest: 2, name: Box::from([]), secret: Box::from([]) },
        },
        Message::Terms { terms: sizes::terms_version(Endpoint::WorkerAgent, &sizes, 2).expect("terms fit") },
        Message::AgentStartV2 {
            start: AgentStart {
                charter: bytes(sizes.charter),
                transcript: Some(bytes(sizes.transcript)),
                repositories,
                endpoints,
                grants,
            },
        },
    ];
    let frames = messages
        .iter()
        .map(|message| codec::encode_version(message, &sizes, 2).expect("maximum body fits"))
        .collect::<Vec<_>>();
    drop(messages);
    let upper = Queue::with_capacity(machine::MAX_UP);
    let lower = Queue::with_capacity(machine::MAX_DOWN);
    let bound = sizes::worst_case_versions(Endpoint::Agent, &limits, &sizes, 1, 2).expect("bound fits");
    let meter = Meter::new();
    let mut machine = Machine::with_versions(Endpoint::Agent, &limits, &sizes, 1, 2).expect("machine fits");
    let mut upper = upper;
    let mut lower = lower;
    machine::poll(&mut machine, &limits, &sizes, &mut upper, &mut lower);
    let mut demand = None;
    while let Some(request) = lower.pop() {
        if let stream::Down::Demand { read, room } = request {
            demand = Some((read, room));
        }
    }
    for frame in &frames {
        let mut at = 0;
        for _ in 0..4096 {
            if at == frame.len() {
                break;
            }
            let (read, room) = demand.take().expect("the next read or credit was demanded");
            let input = if room > 0 {
                stream::Up::Room
            } else {
                let stream::Read::Fill(count) = read else {
                    unreachable!("the sized machine only fills");
                };
                let count = usize::try_from(count).expect("small chunk");
                let input = stream::Up::Bytes(frame[at..at + count].into());
                at += count;
                input
            };
            meter.start();
            machine::up(&mut machine, &limits, &sizes, input, &mut upper, &mut lower);
            let measured = meter.end();
            while let Some(request) = lower.pop() {
                if let stream::Down::Demand { read, room } = request {
                    demand = Some((read, room));
                }
            }
            while let Some(event) = upper.pop() {
                drop(event);
            }
            meter.check(measured, bound, "maximum Start repositories, endpoints and grants");
        }
        assert_eq!(at, frame.len());
    }
    assert_eq!(machine.received_frames(), 3);
    drop(machine);
    assert_eq!(meter.held(), 0);
}

#[test]
fn typed_v2_payload_heaps_and_malformed_counts_are_bounded() {
    use temper_channel::payload::v2;
    let sizes = Sizes { entries: 2, detail: 32, ..Sizes::STARTING };
    let fixtures: &[(&[u8], u8)] = &[
        (include_bytes!("../../../crates/temper-channel/src/payload/golden/charter.bin"), 0),
        (include_bytes!("../../../crates/temper-channel/src/payload/golden/call_0.bin"), 1),
        (include_bytes!("../../../crates/temper-channel/src/payload/golden/inbound.bin"), 2),
        (include_bytes!("../../../crates/temper-channel/src/payload/golden/outcome_1.bin"), 3),
        (include_bytes!("../../../crates/temper-channel/src/payload/golden/served_3.bin"), 4),
        (include_bytes!("../../../crates/temper-channel/src/payload/golden/transcript.bin"), 5),
    ];
    let bound = v2::worst_case(&sizes).expect("bounded typed schema");
    for (bytes, kind) in fixtures {
        let meter = Meter::new();
        meter.start();
        match kind {
            0 => {
                let value = v2::decode_charter(bytes, &sizes).expect("charter golden");
                let measured = meter.end();
                meter.check(measured, bound, "charter");
                drop(value);
            }
            1 => {
                let value = v2::decode_call(bytes, &sizes).expect("call golden");
                let measured = meter.end();
                meter.check(measured, bound, "call");
                drop(value);
            }
            2 => {
                let value = v2::decode_inbound(bytes, &sizes).expect("inbound golden");
                let measured = meter.end();
                meter.check(measured, bound, "inbound");
                drop(value);
            }
            3 => {
                let value = v2::decode_outcome(bytes, &sizes).expect("outcome golden");
                let measured = meter.end();
                meter.check(measured, bound, "outcome");
                drop(value);
            }
            4 => {
                let value = v2::decode_served(bytes, &sizes).expect("served golden");
                let measured = meter.end();
                meter.check(measured, bound, "served");
                drop(value);
            }
            5 => {
                let value = v2::decode_transcript(bytes, &sizes).expect("transcript golden");
                let measured = meter.end();
                meter.check(measured, bound, "transcript");
                drop(value);
            }
            _ => unreachable!("fixture table has six schemas"),
        }
        assert_eq!(meter.held(), 0);
    }
    let meter = Meter::new();
    meter.start();
    assert!(v2::decode_call(&[0, 255, 255, 255, 255], &sizes).is_none());
    let measured = meter.end();
    meter.check(measured, 0, "bad count allocates nothing");
    assert_eq!(meter.held(), 0);
}
