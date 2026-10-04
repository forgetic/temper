use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use temper_channel::{codec, wire};
use temper_worker_domain::agent::channel;
use temper_worker_protocol::{agent, credentials::Table, worst_case};
use temper_worker_protocol_world::sockets::LIMITS;
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
#[test]
fn full_pending_starts_and_two_value_generations_fit_reported_heap() {
    let starts: Vec<_> = (0..LIMITS.agents)
        .map(|_| channel::Down::Start {
            charter: vec![0xff; LIMITS.sizes.charter as usize].into(),
            snapshot: Some(vec![0xfe; LIMITS.sizes.snapshot as usize].into()),
            repositories: (0..LIMITS.sizes.repositories)
                .map(|_| channel::Repository {
                    name: vec![b'n'; LIMITS.sizes.name_bytes as usize].into(),
                    writable: true,
                })
                .collect(),
            grants: Box::new([
                channel::Grant { account: 1, generation: 7, valid: Duration::from_secs(10) },
                channel::Grant { account: 2, generation: 7, valid: Duration::from_secs(10) },
            ]),
        })
        .collect();
    let endpoints: Box<[_]> = (0..LIMITS.sizes.endpoints)
        .map(|endpoint| wire::EndpointDescriptor {
            endpoint,
            provider: wire::Provider::OpenAi,
            host: vec![b'h'; LIMITS.sizes.name_bytes as usize].into(),
            address: wire::Address::V6 { bytes: [7; 16] },
            port: 443,
            path: vec![b'p'; LIMITS.sizes.name_bytes as usize].into(),
            account: endpoint + 1,
            effort: vec![b'e'; LIMITS.sizes.name_bytes as usize].into(),
            thinking: Some(999),
        })
        .collect();
    let mut up = Queue::with_capacity(agent::MAX_UP);
    let mut down = Queue::with_capacity(agent::MAX_DOWN);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS };
    let meter = Meter::new();
    meter.start();
    let mut table = Table::new(&[1, 2], LIMITS.accounts, LIMITS.sizes.token_bytes).expect("table");
    for account in 1..=2 {
        for generation in [6, 7] {
            table
                .insert(
                    wire::Grant {
                        account,
                        generation,
                        valid: Duration::from_secs(10),
                        token: vec![b't'; LIMITS.sizes.token_bytes as usize].into(),
                        account_id: vec![b'i'; LIMITS.sizes.token_bytes as usize].into(),
                    },
                    Time::ZERO,
                    LIMITS.skew,
                )
                .expect("value");
        }
    }
    let mut channels = Vec::new();
    for (index, message) in starts.into_iter().enumerate() {
        let mut channel =
            agent::Channel::new(Token::new(index as u64), Time::from_nanos(5_000_000_000), &LIMITS).expect("channel");
        agent::send(
            &mut channel,
            &env,
            agent::Send { owner: Token::new(100), message },
            &endpoints,
            &table,
            &mut up,
            &mut down,
        )
        .expect("pending start");
        while down.pop().is_some() {}
        while up.pop().is_some() {}
        channels.push(channel);
    }
    let measured = meter.end();
    meter.check(measured, worst_case(&LIMITS).expect("bound"), 40);
    assert_eq!(channels.len(), LIMITS.agents as usize);
}
#[test]
fn frame_measure_does_not_allocate_or_skip_validation() {
    let message = wire::Message::AgentEvent { event: 999, body: vec![7; LIMITS.sizes.inbound as usize].into() };
    let meter = Meter::new();
    meter.start();
    let length = codec::frame_len(&message, &LIMITS.sizes).expect("fits");
    let measured = meter.end();
    meter.check(measured, 0, 41);
    assert_eq!(length, 12 + 8 + LIMITS.sizes.inbound);
}
