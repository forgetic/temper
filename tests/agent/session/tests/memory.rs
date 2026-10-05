//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the session child domain with every session filled to
//! its limits, and lib's containers on their own.

use std::mem::size_of;

use skein_lib::{Deadlines, Duration, Env, List, Map, Queue, Rng, Set, Slab, Time, Token, Wall};
use temper_agent_domain_session::llm::{
    Block, Completion, Decoded, Descriptor, Endpoint, Failure, Problem, Stop, Usage,
};
use temper_agent_domain_session::{Budget, Domain, Event, Limits, MAX_PARALLEL, Request, Spec, max_out, worst_case};
use temper_agent_domain_tools::{Authority, Call, Done, Effect, Grants, Name, Part, Path, Repo, Version};
use temper_agent_session_world::TOOLS;
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

fn bytes(len: u64) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

/// The one name the authority and the call use.
fn name() -> Name {
    Name::new(bytes(1)).expect("a name")
}

/// What the session's tools may do: inspect one repository, mounted at a
/// one-byte name.
fn authority() -> Authority {
    Authority {
        cwd: Box::new([name()]),
        repos: Box::new([Repo { mount: Box::new([name()]), root: Token::new(1), writable: false }]),
        grants: Grants { inspect: true, modify: false, shell: false },
        env: Box::new([]),
    }
}

const VERSION: Version = Version::new([1, 0, 0, 0]);

fn size(of: usize) -> u64 {
    u64::try_from(of).expect("a size fits")
}

const LIMITS: Limits = Limits {
    sessions: 1,
    spend: 0,
    messages: 4,
    session_bytes: 1024,
    budget: Budget {
        turns: 16,
        input: 1 << 20,
        output: 1 << 20,
        cache_read: 1 << 20,
        cache_write: 1 << 20,
        time: Duration::from_secs(3600),
    },
    max_tokens: 1024,
    retries: 1,
    backoff_base: Duration::from_secs(60),
    backoff_max: Duration::from_secs(60),
    call_timeout: Duration::from_secs(30),
    tool_timeout: Duration::from_secs(20),
    facts: 64,
    parallel_tools: 1,
    // Reads answer with the whole file loaded.
    tools: temper_agent_domain_tools::Limits { kits: 1, read_bytes: 1 << 20, file_bytes: 1 << 20, ..TOOLS },
};

/// What a step asked for last, without the payload.
enum Asked {
    Complete { owner: Token },
    Io { owner: Token },
    Other,
}

/// How a session fills its bytes.
#[derive(Clone, Copy, Debug)]
enum Route {
    /// The LLM calls a tool, whose output fills the rest.
    Tool,
    /// The LLM makes a call that cannot be decoded, whose problem, and the
    /// answer that repeats it, fill the rest.
    Invalid,
    /// The LLM yields, and the opener's next message fills the rest.
    Talk,
}

/// Fills every session of a domain under `limits` to exactly its byte limit by
/// `route` and leaves it in backoff, the state that also holds both of its
/// alarms, checking the peak of the heap in every step against the worst case.
/// Its
/// tools have a kit for each session, and room for its widest batch.
fn fill(limits: Limits, route: Route) {
    let tools = temper_agent_domain_tools::Limits {
        kits: limits.sessions,
        calls: limits.parallel_tools.max(limits.tools.calls),
        ..limits.tools
    };
    let limits = Limits { tools, ..limits };
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let meter = Meter::new();
    let mut domain = Domain::new(&limits, 1);
    // The requests are the protocol layer's and the opener's to hold and
    // count: each is dropped, keeping only what it asked for, and the step's
    // peak checked less them.
    let mut step = |event: Event| -> Option<Asked> {
        meter.start();
        temper_agent_domain_session::step(&mut domain, &env, event, &mut out);
        let measured = meter.end();
        let mut asked = None;
        while let Some(request) = out.pop() {
            asked = Some(match request {
                Request::Complete { owner, .. } => Asked::Complete { owner },
                Request::Io { owner, .. } => Asked::Io { owner },
                Request::Opened { .. }
                | Request::Yielded { .. }
                | Request::Used { .. }
                | Request::Ended { .. }
                | Request::Cancel { .. }
                | Request::CancelIo { .. }
                | Request::Delegate { .. }
                | Request::Withdraw { .. }
                | Request::Turn { .. }
                | Request::Priced { .. } => Asked::Other,
            });
        }
        meter.check(measured, bound, limits);
        asked
    };
    let (block, part) = (size(size_of::<Block>()), size(size_of::<Part>()));
    for opener in 0..limits.sessions {
        // What the domain charges, as it charges it: the spec's names, the
        // tools its opener serves and its prompt; then, by tool, the
        // assistant's message with its call and room for its result with the
        // result's id, then the result's output; by an invalid
        // call, the message with its problem and room for its answer, then
        // the answer's id and the problem again; or, by talk, the assistant's
        // answer, then the opener's message.
        let spec = Spec {
            endpoint: Endpoint(0),
            model: bytes(1),
            system: bytes(1),
            authority: authority(),
            delegated: Box::new([
                Descriptor { ticket: Token::new(1), effect: Effect::Write },
                Descriptor { ticket: Token::new(2), effect: Effect::Read },
            ]),
            prompt: bytes(1),
            max_tokens: 1,
            budget: limits.budget,
        };
        let spec_cost = 2 + 2 * size(size_of::<Descriptor>()) + (block + 1);

        let opener = Token::new(u64::from(opener));
        let Some(Asked::Complete { owner }) = step(Event::Open { opener, spec }) else {
            panic!("the spec fits the limits");
        };
        match route {
            Route::Tool => {
                // A call's slot for its result takes a block's room, and its
                // result's id is charged with it.
                let tooling_cost = (block + 3 + (part + 1)) + (block + 1);
                let output = limits.session_bytes - spec_cost - tooling_cost;
                let path = Path { absolute: false, parts: Box::new([Part::Name { name: name() }]) };
                let call = Decoded::Owned { call: Call::Read { path, skip: 0, lines: None } };
                let content = Box::new([Block::ToolCall { id: bytes(1), name: bytes(1), input: bytes(1), call }]);
                let completion = Completion { content, stop: Stop::ToolUse, usage: Usage::ZERO };
                let Some(Asked::Io { owner: op }) = step(Event::Completed { owner, completion }) else {
                    panic!("the session's tools load the file");
                };
                let done = Event::Done { owner: op, done: Done::Loaded { content: bytes(output), version: VERSION } };
                let Some(Asked::Complete { .. }) = step(done) else {
                    panic!("a session filled exactly to its byte limit goes on");
                };
            }
            Route::Invalid => {
                // The problem is held twice, in the call and in its answer.
                let field = (limits.session_bytes - spec_cost - (block + 3) - (block + 1)) / 2;
                let call = Decoded::Invalid { problem: Problem::Missing { field: bytes(field) } };
                let content = Box::new([Block::ToolCall { id: bytes(1), name: bytes(1), input: bytes(1), call }]);
                let completion = Completion { content, stop: Stop::ToolUse, usage: Usage::ZERO };
                let Some(Asked::Complete { .. }) = step(Event::Completed { owner, completion }) else {
                    panic!("a session that answers its calls itself goes on");
                };
            }
            Route::Talk => {
                let answer_cost = block + 1;
                let message = limits.session_bytes - spec_cost - answer_cost - block;
                let content = Box::new([Block::Text { text: bytes(1) }]);
                let completion = Completion { content, stop: Stop::EndTurn, usage: Usage::ZERO };
                let Some(Asked::Other) = step(Event::Completed { owner, completion }) else {
                    panic!("the session yields");
                };
                let Some(Asked::Complete { .. }) = step(Event::Continue { session: owner, content: bytes(message) })
                else {
                    panic!("a session filled exactly to its byte limit goes on");
                };
            }
        }
        let backoff = step(Event::Failed { owner, failure: Failure::Overloaded });
        assert!(backoff.is_none(), "a transient failure backs off quietly");
    }
    let held = meter.held();
    let full = u64::from(limits.sessions) * limits.session_bytes;
    assert!(held >= full, "{limits:?}: every session holds its byte limit");
}

#[test]
fn a_domain_with_every_session_full_stays_within_its_worst_case() {
    for route in [Route::Tool, Route::Invalid, Route::Talk] {
        fill(LIMITS, route);
        fill(Limits { sessions: 64, spend: 0, session_bytes: 65_536, ..LIMITS }, route);
        fill(Limits { sessions: 1000, spend: 0, messages: 8, session_bytes: 600, ..LIMITS }, route);
        fill(Limits { sessions: 64, spend: 0, parallel_tools: MAX_PARALLEL, ..LIMITS }, route);
    }
}

/// Arms, re-arms, cancels and fires timers at random in a table of `capacity`,
/// checking the peak of its heap in every change against its worst case. The
/// bound
/// leans on how the standard library builds its B-trees, which this checks.
fn churn<K: Ord + Copy>(capacity: u32, seed: u64, key: fn(u64) -> K) {
    let bound = Deadlines::<K>::worst_case(capacity).expect("a test capacity fits");
    let mut rng = Rng::new(seed);
    let meter = Meter::new();
    let mut timers = Deadlines::with_capacity(capacity);
    let keys = u64::from(capacity) * 2;
    for round in 0..u64::from(capacity) * 20 {
        meter.start();
        match rng.below(8) {
            0..=4 => drop(timers.arm(key(rng.below(keys)), Time::from_nanos(round + rng.below(keys)))),
            5 | 6 => timers.cancel(key(rng.below(keys))),
            _ => drop(timers.expire(Time::from_nanos(round))),
        }
        meter.check(meter.end(), bound, format_args!("{capacity} timers"));
    }
}

#[test]
fn a_deadline_table_stays_within_its_worst_case_whatever_its_keys_and_order() {
    for capacity in [1, 2, 11, 64, 1000] {
        for seed in 0..4 {
            churn(capacity, seed, |n| u8::try_from(n % 256).expect("below 256"));
            churn(capacity, seed, |n| (u32::try_from(n).expect("a small key"), 7_u32));
            churn(capacity, seed, |n| [n; 4]);
        }
    }
}

/// Inserts, replaces, updates and removes entries at random in a map of
/// `capacity`, checking the peak of its heap in every change against its worst
/// case. `payload` is the heap each entry's key and value own, which is the
/// owner's to count: the bound adds it for each entry held, and for the key
/// handed in.
fn traffic<K: Ord, V>(capacity: u32, seed: u64, key: fn(u64) -> K, value: fn(u64) -> V, payload: u64) {
    let bound = Map::<K, V>::worst_case(capacity).expect("a test capacity fits");
    let mut rng = Rng::new(seed);
    let meter = Meter::new();
    let mut map = Map::with_capacity(capacity);
    let keys = u64::from(capacity) * 2;
    for _ in 0..u64::from(capacity) * 20 {
        let owned = (u64::from(map.len()) + 1) * payload;
        meter.start();
        match rng.below(8) {
            0..=4 => drop(map.insert(key(rng.below(keys)), value(rng.next_u64()))),
            5 => {
                if let Some(slot) = map.get_mut(&key(rng.below(keys))) {
                    *slot = value(rng.next_u64());
                }
            }
            _ => drop(map.remove(&key(rng.below(keys)))),
        }
        meter.check(meter.end(), bound + owned, format_args!("{capacity} entries"));
    }
}

/// The same for a set.
fn members<K: Ord>(capacity: u32, seed: u64, key: fn(u64) -> K) {
    let bound = Set::<K>::worst_case(capacity).expect("a test capacity fits");
    let mut rng = Rng::new(seed);
    let meter = Meter::new();
    let mut set = Set::with_capacity(capacity);
    let keys = u64::from(capacity) * 2;
    for _ in 0..u64::from(capacity) * 20 {
        meter.start();
        if rng.chance(600) {
            drop(set.insert(key(rng.below(keys))));
        } else {
            let _: bool = set.remove(&key(rng.below(keys)));
        }
        meter.check(meter.end(), bound, format_args!("{capacity} keys"));
    }
}

#[test]
fn maps_and_sets_stay_within_their_worst_case_whatever_their_keys_and_values() {
    for capacity in [1, 2, 11, 64, 1000] {
        for seed in 0..4 {
            traffic(capacity, seed, |n| u8::try_from(n % 256).expect("below 256"), |_| (), 0);
            traffic(capacity, seed, |n| (u32::try_from(n).expect("a small key"), 7_u32), |n| n, 0);
            traffic(capacity, seed, |n| [n; 4], |n| [n.to_be_bytes()[7]; 3], 0);
            // Keys that own their bytes, as paths do: eight each.
            traffic(capacity, seed, |n| Box::<[u8]>::from(n.to_be_bytes()), |n| n, 8);
            members(capacity, seed, |n| u16::try_from(n).expect("a small key"));
            members(capacity, seed, |n| [n; 3]);
        }
    }
}

#[test]
fn slabs_lists_and_queues_take_no_more_than_their_worst_case() {
    for capacity in [0, 1, 100] {
        let meter = Meter::new();
        meter.start();
        let slab: Slab<[u64; 5]> = Slab::with_capacity(capacity);
        let bound = Slab::<[u64; 5]>::worst_case(capacity).expect("fits");
        meter.check(meter.end(), bound, format_args!("a slab of {capacity}"));
        drop(slab);
        meter.start();
        let list: List<[u64; 5]> = List::with_capacity(capacity);
        let bound = List::<[u64; 5]>::worst_case(capacity).expect("fits");
        meter.check(meter.end(), bound, format_args!("a list of {capacity}"));
        drop(list);
        meter.start();
        let queue: Queue<[u64; 5]> = Queue::with_capacity(capacity);
        let bound = Queue::<[u64; 5]>::worst_case(capacity).expect("fits");
        meter.check(meter.end(), bound, format_args!("a queue of {capacity}"));
        drop(queue);
    }
}

#[test]
fn recorded_delegated_turns_hold_exactly_the_byte_cap_and_count_their_copies() {
    use temper_agent_domain_session::record;
    let limits = Limits { messages: 6, spend: u64::MAX, ..LIMITS };
    let bound = worst_case(&limits).expect("the counted scenario fits");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let meter = Meter::new();
    let mut domain = Domain::new(&limits, 77);
    let mut drive = |event| {
        meter.start();
        temper_agent_domain_session::step(&mut domain, &env, event, &mut out);
        let measured = meter.end();
        let mut completion = None;
        let mut delegate = None;
        while let Some(request) = out.pop() {
            match request {
                Request::Complete { owner, .. } => completion = Some(owner),
                Request::Delegate { owner, .. } => delegate = Some(owner),
                Request::Opened { .. }
                | Request::Yielded { .. }
                | Request::Used { .. }
                | Request::Ended { .. }
                | Request::Turn { .. }
                | Request::Priced { .. }
                | Request::Cancel { .. }
                | Request::Io { .. }
                | Request::CancelIo { .. }
                | Request::Withdraw { .. } => {}
            }
        }
        meter.check(measured, bound, limits);
        (completion, delegate)
    };
    let spec = Spec {
        endpoint: Endpoint(7),
        model: bytes(1),
        system: bytes(1),
        authority: authority(),
        delegated: Box::new([Descriptor { ticket: Token::new(2), effect: Effect::Write }]),
        prompt: bytes(1),
        max_tokens: 1,
        budget: limits.budget,
    };
    let block = size(size_of::<Block>());
    let spec_charge = 2 + size(size_of::<Descriptor>()) + block + 1;
    let turn_charge = block + 1 + block + 3 + block + 1;
    let output = limits.session_bytes - spec_charge - turn_charge;
    let (owner, _) = drive(Event::OpenV2 {
        opener: Token::new(1),
        spec: record::Opening {
            spec,
            dialect: 2,
            prices: record::Prices { input: 1, cached: 1, output: 1, unit: 1 },
            budget: 1,
            transcript: None,
        },
    });
    let (_, delegate) = drive(Event::Completed {
        owner: owner.expect("the counted scenario fits"),
        completion: Completion {
            content: Box::new([
                Block::Opaque { bytes: bytes(1) },
                Block::ToolCall {
                    id: bytes(1),
                    name: bytes(1),
                    input: bytes(1),
                    call: Decoded::Delegated { ticket: Token::new(99), effect: Effect::Write },
                },
            ]),
            stop: Stop::ToolUse,
            usage: Usage::ZERO,
        },
    });
    let (owner, _) = drive(Event::AnsweredV2 {
        owner: delegate.expect("the counted scenario fits"),
        text: bytes(output),
        error: false,
        spent: 0,
    });
    drive(Event::Failed { owner: owner.expect("the counted scenario fits"), failure: Failure::Overloaded });
    assert!(meter.held() >= limits.session_bytes);
}

#[test]
fn restoring_a_maximum_recorded_history_stays_within_the_counted_bound() {
    use temper_agent_domain_session::record;
    let limits = Limits { messages: 6, spend: u64::MAX, ..LIMITS };
    let bound = worst_case(&limits).expect("the limits fit");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let meter = Meter::new();
    let mut domain = Domain::new(&limits, 78);
    let block = size(size_of::<Block>());
    let charge = 2 + size(size_of::<Descriptor>()) + block + 1;
    let opaque = limits.session_bytes - charge - 2 * block - 1;
    let spec = Spec {
        endpoint: Endpoint(7),
        model: bytes(1),
        system: bytes(1),
        authority: authority(),
        delegated: Box::new([Descriptor { ticket: Token::new(2), effect: Effect::Write }]),
        prompt: bytes(1),
        max_tokens: 1,
        budget: limits.budget,
    };
    let history = record::Transcript {
        version: record::VERSION,
        endpoint: Endpoint(7),
        dialect: 2,
        turns: Box::new([record::Turn {
            version: record::VERSION,
            endpoint: Endpoint(7),
            dialect: 2,
            sequence: 1,
            usage: Usage::ZERO,
            spent: 0,
            messages: Box::new([
                temper_agent_domain_session::llm::Message {
                    role: temper_agent_domain_session::llm::Role::User,
                    content: Box::new([Block::Text { text: bytes(1) }]),
                },
                temper_agent_domain_session::llm::Message {
                    role: temper_agent_domain_session::llm::Role::Assistant,
                    content: Box::new([Block::Opaque { bytes: bytes(opaque) }]),
                },
            ]),
        }]),
        after: Box::default(),
    };
    meter.start();
    temper_agent_domain_session::step(
        &mut domain,
        &env,
        Event::OpenV2 {
            opener: Token::new(1),
            spec: record::Opening {
                spec,
                dialect: 2,
                prices: record::Prices { input: 1, cached: 1, output: 1, unit: 1 },
                budget: 1,
                transcript: Some(history),
            },
        },
        &mut out,
    );
    let measured = meter.end();
    let mut completed = false;
    while let Some(request) = out.pop() {
        if let Request::Complete { .. } = request {
            completed = true;
        }
    }
    assert!(completed, "a history filled to the exact byte cap still leaves an answer slot");
    meter.check(measured, bound, limits);
    assert!(meter.held() >= limits.session_bytes);
}
