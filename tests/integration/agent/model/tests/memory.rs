//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the model with every session filled to its limits,
//! and lib's containers on their own.

use std::mem::size_of;

use temper_agent_model::llm::{Block, Completion, Endpoint, Failure, Stop, Tool, Usage};
use temper_agent_model::{Event, Limits, MAX_OUT, Model, Request, Task, worst_case};
use temper_lib::{Deadlines, Duration, Env, List, Queue, ReplyTo, Rng, Slab, Time, Token};

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

fn size(of: usize) -> u64 {
    u64::try_from(of).expect("a size fits")
}

const LIMITS: Limits = Limits {
    sessions: 1,
    messages: 3,
    session_bytes: 1024,
    turns: 16,
    max_tokens: 1024,
    retries: 1,
    backoff_base: Duration::from_secs(60),
    backoff_max: Duration::from_secs(60),
    call_timeout: Duration::from_secs(30),
    session_timeout: Duration::from_secs(3600),
};

/// What a step asked for, without the payload.
enum Asked {
    Complete { owner: Token },
    Tool,
    Other,
}

/// Fills every session of a model under `limits` to exactly its byte limit and
/// leaves it in backoff, the state that also holds both of its alarms, checking
/// the heap against the worst case after every step.
fn fill(limits: Limits) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, limits };
    let mut out = Queue::with_capacity(MAX_OUT);
    let base = heap::live();
    let mut model = Model::new(&limits, 1);
    // The requests are the protocol layer's to hold and count: each is
    // dropped, keeping only what it asked for, before the heap is measured.
    let mut step = |event: Event| -> Option<Asked> {
        temper_agent_model::step(&mut model, &env, event, &mut out);
        let asked = match out.pop()? {
            Request::Complete { owner, .. } => Asked::Complete { owner },
            Request::Tool { .. } => Asked::Tool,
            Request::Reply { .. } | Request::Cancel { .. } | Request::CancelTool { .. } => Asked::Other,
        };
        let held = held(base);
        assert!(held <= bound, "{limits:?}: the model holds {held} bytes, more than its worst case of {bound}");
        Some(asked)
    };
    let (block, tool) = (size(size_of::<Block>()), size(size_of::<Tool>()));
    for run in 0..limits.sessions {
        // What the model charges, as it charges it: the task's names, tools
        // and prompt; then the assistant's message and room for its result;
        // then the result's id and output.
        let task = Task {
            endpoint: Endpoint(0),
            model: bytes(1),
            system: bytes(1),
            tools: Box::new([Tool { name: bytes(1), description: bytes(1), schema: bytes(1) }]),
            prompt: bytes(1),
            max_tokens: 1,
        };
        let task_cost = 2 + (tool + 3) + (block + 1);
        let tooling_cost = (block + 3) + block;
        let output = limits.session_bytes - task_cost - tooling_cost - 1;

        let reply_to = ReplyTo::new(Token::new(u64::from(run)));
        let Some(Asked::Complete { owner }) = step(Event::Run { reply_to, task }) else {
            panic!("the task fits the limits");
        };
        let content = Box::new([Block::ToolCall { id: bytes(1), name: bytes(1), input: bytes(1) }]);
        let completion = Completion { content, stop: Stop::ToolUse, usage: Usage::ZERO };
        let Some(Asked::Tool) = step(Event::Completed { owner, completion }) else {
            panic!("the session runs the tool");
        };
        let Some(Asked::Complete { .. }) = step(Event::ToolDone { owner, output: bytes(output), error: false }) else {
            panic!("a session filled exactly to its byte limit goes on");
        };
        let backoff = step(Event::Failed { owner, failure: Failure::Overloaded });
        assert!(backoff.is_none(), "a transient failure backs off quietly");
    }
    let held = held(base);
    let full = u64::from(limits.sessions) * limits.session_bytes;
    assert!(held >= full, "{limits:?}: every session holds its byte limit");
}

#[test]
fn a_model_with_every_session_full_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { sessions: 64, session_bytes: 65_536, ..LIMITS });
    fill(Limits { sessions: 1000, messages: 8, session_bytes: 600, ..LIMITS });
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
