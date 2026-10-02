//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the session sub-model with every session filled to its
//! limits, and lib's containers on their own.

use std::mem::size_of;

use temper_agent_model_session::llm::{
    Block, Completion, Decoded, Descriptor, Endpoint, Failure, Problem, Stop, Usage,
};
use temper_agent_model_session::{Budget, Event, Limits, MAX_PARALLEL, Model, Request, Spec, max_out, worst_case};
use temper_agent_model_session_tests::TOOLS;
use temper_agent_model_tools::{Authority, Call, Done, Effect, Grants, Name, Part, Path, Repo, Version};
use temper_lib::{Deadlines, Duration, Env, List, Map, Queue, Rng, Set, Slab, Time, Token};

/// Counts the heap each thread allocates, so that tests running side by side
/// do not see each other's.
#[expect(unsafe_code, reason = "a global allocator is an unsafe impl; it only counts, and System allocates")]
mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    #[global_allocator]
    static COUNTING: Counting = Counting;

    thread_local! {
        // Const-initialised and without a destructor: reading it never
        // allocates, so the allocator can use it.
        static LIVE: Cell<i64> = const { Cell::new(0) };
    }

    fn count(layout: Layout, sign: i64) {
        let size = i64::try_from(layout.size()).unwrap_or(i64::MAX);
        LIVE.with(|live| live.set(live.get().wrapping_add(size.wrapping_mul(sign))));
    }

    // SAFETY: every call is passed to System unchanged; counting touches no
    // memory the caller sees. realloc and alloc_zeroed keep their default
    // bodies, which call these two.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count(layout, 1);
            // SAFETY: the caller upholds alloc's contract, which is System's.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            count(layout, -1);
            // SAFETY: `ptr` came from System.alloc with `layout`, above.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    /// Heap this thread allocated and has not freed, in bytes.
    pub fn live() -> i64 {
        LIVE.with(Cell::get)
    }
}

/// Bytes allocated on this thread since `base` and not freed.
fn held(base: i64) -> u64 {
    u64::try_from(heap::live() - base).expect("nothing freed that was not allocated since the base")
}

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
    delegate_timeout: Duration::from_secs(40),
    facts: 64,
    parallel_tools: 1,
    // Reads answer with the whole file loaded.
    tools: temper_agent_model_tools::Limits { kits: 1, read_bytes: 1 << 20, file_bytes: 1 << 20, ..TOOLS },
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

/// Fills every session of a model under `limits` to exactly its byte limit by
/// `route` and leaves it in backoff, the state that also holds both of its
/// alarms, checking the heap against the worst case after every step. Its
/// tools have a kit for each session, and room for its widest batch.
fn fill(limits: Limits, route: Route) {
    let tools = temper_agent_model_tools::Limits {
        kits: limits.sessions,
        calls: limits.parallel_tools.max(limits.tools.calls),
        ..limits.tools
    };
    let limits = Limits { tools, ..limits };
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let base = heap::live();
    let mut model = Model::new(&limits, 1);
    // The requests are the protocol layer's and the opener's to hold and
    // count: each is dropped, keeping only what it asked for, before the heap
    // is measured.
    let mut step = |event: Event| -> Option<Asked> {
        temper_agent_model_session::step(&mut model, &env, event, &mut out);
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
                | Request::Withdraw { .. } => Asked::Other,
            });
        }
        let held = held(base);
        assert!(held <= bound, "{limits:?}: the model holds {held} bytes, more than its worst case of {bound}");
        asked
    };
    let (block, part) = (size(size_of::<Block>()), size(size_of::<Part>()));
    for opener in 0..limits.sessions {
        // What the model charges, as it charges it: the spec's names, the
        // tools its opener serves and its prompt; then, by tool, the assistant's message with its call and
        // room for its result, then the result's id and output; by an invalid
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
                // A call's slot for its result takes a block's room.
                let tooling_cost = (block + 3 + (part + 1)) + block;
                let output = limits.session_bytes - spec_cost - tooling_cost - 1;
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
    let held = held(base);
    let full = u64::from(limits.sessions) * limits.session_bytes;
    assert!(held >= full, "{limits:?}: every session holds its byte limit");
}

#[test]
fn a_model_with_every_session_full_stays_within_its_worst_case() {
    for route in [Route::Tool, Route::Invalid, Route::Talk] {
        fill(LIMITS, route);
        fill(Limits { sessions: 64, session_bytes: 65_536, ..LIMITS }, route);
        fill(Limits { sessions: 1000, messages: 8, session_bytes: 600, ..LIMITS }, route);
        fill(Limits { sessions: 64, parallel_tools: MAX_PARALLEL, ..LIMITS }, route);
    }
}

/// Arms, re-arms, cancels and fires timers at random in a table of `capacity`,
/// checking its heap against its worst case after every change. The bound
/// leans on how the standard library builds its B-trees, which this checks.
fn churn<K: Ord + Copy>(capacity: u32, seed: u64, key: fn(u64) -> K) {
    let bound = Deadlines::<K>::worst_case(capacity).expect("a test capacity fits");
    let mut rng = Rng::new(seed);
    let base = heap::live();
    let mut timers = Deadlines::with_capacity(capacity);
    let keys = u64::from(capacity) * 2;
    for round in 0..u64::from(capacity) * 20 {
        match rng.below(8) {
            0..=4 => drop(timers.arm(key(rng.below(keys)), Time::from_nanos(round + rng.below(keys)))),
            5 | 6 => timers.cancel(key(rng.below(keys))),
            _ => drop(timers.expire(Time::from_nanos(round))),
        }
        let held = held(base);
        assert!(held <= bound, "{capacity} timers hold {held} bytes, more than their worst case of {bound}");
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
/// `capacity`, checking its heap against its worst case after every change.
/// `payload` is the heap each entry's key and value own, which is the owner's
/// to count: the bound adds it per entry held.
fn traffic<K: Ord, V>(capacity: u32, seed: u64, key: fn(u64) -> K, value: fn(u64) -> V, payload: u64) {
    let bound = Map::<K, V>::worst_case(capacity).expect("a test capacity fits");
    let mut rng = Rng::new(seed);
    let base = heap::live();
    let mut map = Map::with_capacity(capacity);
    let keys = u64::from(capacity) * 2;
    for _ in 0..u64::from(capacity) * 20 {
        match rng.below(8) {
            0..=4 => drop(map.insert(key(rng.below(keys)), value(rng.next_u64()))),
            5 => {
                if let Some(slot) = map.get_mut(&key(rng.below(keys))) {
                    *slot = value(rng.next_u64());
                }
            }
            _ => drop(map.remove(&key(rng.below(keys)))),
        }
        let held = held(base);
        let owned = u64::from(map.len()) * payload;
        assert!(
            held <= bound + owned,
            "{capacity} entries hold {held} bytes, more than their worst case of {bound} and {owned}"
        );
    }
}

/// The same for a set.
fn members<K: Ord>(capacity: u32, seed: u64, key: fn(u64) -> K) {
    let bound = Set::<K>::worst_case(capacity).expect("a test capacity fits");
    let mut rng = Rng::new(seed);
    let base = heap::live();
    let mut set = Set::with_capacity(capacity);
    let keys = u64::from(capacity) * 2;
    for _ in 0..u64::from(capacity) * 20 {
        if rng.chance(600) {
            drop(set.insert(key(rng.below(keys))));
        } else {
            let _: bool = set.remove(&key(rng.below(keys)));
        }
        let held = held(base);
        assert!(held <= bound, "{capacity} keys hold {held} bytes, more than their worst case of {bound}");
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
        let base = heap::live();
        let slab: Slab<[u64; 5]> = Slab::with_capacity(capacity);
        assert!(held(base) <= Slab::<[u64; 5]>::worst_case(capacity).expect("fits"), "a slab of {capacity}");
        drop(slab);
        let list: List<[u64; 5]> = List::with_capacity(capacity);
        assert!(held(base) <= List::<[u64; 5]>::worst_case(capacity).expect("fits"), "a list of {capacity}");
        drop(list);
        let queue: Queue<[u64; 5]> = Queue::with_capacity(capacity);
        assert!(held(base) <= Queue::<[u64; 5]>::worst_case(capacity).expect("fits"), "a queue of {capacity}");
        drop(queue);
    }
}
