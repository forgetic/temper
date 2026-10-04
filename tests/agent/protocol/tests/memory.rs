use skein_lib::{Duration, Env, Queue, Time, Token, Wall, stream};
use temper_agent_domain::run::charter::{Checkout, Repository};
use temper_agent_protocol::channel::{self, Channel, Up};
use temper_agent_protocol_world::{fixture, pipe};
use temper_channel::{codec, machine::Endpoint, payload, sizes, wire};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

// Source tapes belong to the peer and are prepared before the meter. This
// ledger delivers exact demanded bytes and debits every output Send.
struct Ledger {
    demand: Option<(stream::Read, u32)>,
    credit: u32,
}
impl Ledger {
    fn feed(
        &mut self,
        channel: &mut Channel,
        env: &Env<channel::Limits>,
        bytes: &[u8],
        out: &mut Queue<Up>,
        lower: &mut Queue<stream::Down>,
    ) {
        let mut at = 0;
        for _ in 0..20_000 {
            while let Some(value) = lower.pop() {
                match value {
                    stream::Down::Demand { read, room } => {
                        if read == stream::Read::Nothing && room == 0 {
                            self.demand = None;
                        } else {
                            assert!(self.demand.is_none());
                            self.demand = Some((read, room));
                        }
                    }
                    stream::Down::Send(bytes) => {
                        let n = u32::try_from(bytes.len()).expect("frame");
                        assert!(n <= self.credit);
                        self.credit -= n;
                    }
                    stream::Down::Finish => {}
                }
            }
            if let Some((read, room)) = self.demand {
                if room > 0 {
                    self.demand = None;
                    self.credit = room;
                    channel::up(channel, env, stream::Up::Room, out, lower);
                    continue;
                }
                let n = match read {
                    stream::Read::Fill(n) => n as usize,
                    stream::Read::Nothing => 0,
                    stream::Read::Scan { .. } | stream::Read::Line { .. } => panic!("exact frame reads"),
                };
                if n > 0 && bytes.len() - at >= n {
                    self.demand = None;
                    channel::up(channel, env, stream::Up::Bytes(bytes[at..at + n].into()), out, lower);
                    at += n;
                    continue;
                }
            }
            if channel.is_ready() {
                channel::resume(channel, env, out, lower);
                continue;
            }
            assert_eq!(at, bytes.len());
            return;
        }
        panic!("bounded progress");
    }
}
#[test]
fn maximal_deferred_start_endpoint_values_roots_and_conversion_fit_owner_bound() {
    let limits = pipe::limits();
    let open = codec::encode(
        &wire::Message::Open {
            open: wire::Open {
                channel: wire::Channel::Agent,
                lowest: 1,
                highest: 1,
                name: Box::new([]),
                secret: Box::new([]),
            },
        },
        &limits.sizes,
    )
    .expect("open");
    let terms = codec::encode(
        &wire::Message::Terms { terms: sizes::terms(Endpoint::WorkerAgent, &limits.sizes).expect("terms") },
        &limits.sizes,
    )
    .expect("terms frame");
    let mut charter = pipe::charter();
    charter.instructions = vec![b'i'; limits.protocol.request_bytes as usize - 64].into();
    charter.models = (0..limits.protocol.endpoints)
        .map(|endpoint| payload::Model {
            endpoint,
            model: vec![b'a' + u8::try_from(endpoint).expect("four models"); limits.sizes.name_bytes as usize].into(),
            max_tokens: 100,
        })
        .collect();
    let endpoints = (0..limits.protocol.endpoints)
        .map(|endpoint| {
            let mut value = fixture::endpoint(wire::Provider::OpenAi);
            value.endpoint = endpoint;
            value.account = endpoint % limits.protocol.accounts;
            value.host = vec![b'h'; limits.protocol.name_bytes as usize].into();
            value.path = vec![b'/'; limits.protocol.name_bytes as usize].into();
            value.effort = vec![b'e'; limits.protocol.name_bytes as usize].into();
            value
        })
        .collect();
    let repositories: Box<[_]> = (0..limits.sizes.repositories)
        .map(|index| wire::AgentRepository {
            name: vec![b'a' + u8::try_from(index).expect("eight roots"); limits.sizes.name_bytes as usize].into(),
            writable: index % 2 == 0,
        })
        .collect();
    let checkout = Checkout {
        repositories: repositories
            .iter()
            .map(|r| Repository { name: r.name.clone(), root: Token::new(9), writable: r.writable })
            .collect(),
    };
    let grants = (0..limits.protocol.accounts)
        .map(|account| wire::Grant {
            account,
            generation: 1,
            valid: Duration::from_secs(10),
            token: vec![b't'; limits.protocol.token_bytes as usize].into(),
            account_id: vec![b'i'; limits.protocol.token_bytes as usize].into(),
        })
        .collect();
    let start = codec::encode(
        &wire::Message::AgentStart {
            charter: payload::encode_charter(&charter, &limits.sizes).expect("charter"),
            snapshot: None,
            repositories,
            endpoints,
            grants,
        },
        &limits.sizes,
    )
    .expect("start frame");
    let meter = Meter::new();
    meter.start();
    let mut channel = Channel::new(Token::new(5), Time::ZERO, &limits, Duration::ZERO).expect("bounded owner");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::with_capacity(channel::MAX_UP);
    let mut lower = Queue::with_capacity(channel::MAX_DOWN);
    let mut ledger = Ledger { demand: None, credit: 0 };
    ledger.feed(&mut channel, &env, &open, &mut out, &mut lower);
    ledger.feed(&mut channel, &env, &terms, &mut out, &mut lower);
    ledger.feed(&mut channel, &env, &start, &mut out, &mut lower);
    assert_eq!(channel.state(), channel::State::Rooting);
    // Borrowed outer owner data is copied at this actual handoff.
    channel::roots(&mut channel, &env, checkout.clone(), &mut out).expect("roots");
    let measured = meter.end();
    meter.check(measured, channel::worst_case(&limits).expect("owner heap bound"), "maximal agent entrance");
    assert_eq!(channel.endpoints().len(), limits.protocol.endpoints as usize);
    assert_eq!(channel.credentials().names(Time::ZERO).len(), limits.protocol.accounts as usize);
}
#[test]
fn overflowing_owner_limits_are_refused_before_allocating() {
    let mut limits = pipe::limits();
    limits.sizes.facts = u32::MAX;
    limits.sizes.fact = u32::MAX;
    assert!(channel::worst_case(&limits).is_none());
    assert!(Channel::new(Token::new(1), Time::ZERO, &limits, Duration::ZERO).is_none());
}
