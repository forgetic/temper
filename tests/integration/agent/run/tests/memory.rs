//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the run sub-model with every run holding a charter of
//! exactly its byte limit, and every conversation started and spending.

use std::mem::size_of;

use temper_agent_model_run::charter::{Checkout, Endpoint, Grants, Llm, Outlet, Repository, Tools};
use temper_agent_model_run::outcome::{Children, OutcomeSpec, VerdictRule};
use temper_agent_model_run::{
    Answer, Budget, Charter, Event, Invalid, Limits, MAX_OUT, Model, Refusal, Request, Spend, Stop, worst_case,
};
use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};

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

const BUDGET: Budget = Budget {
    turns: 10,
    input: 1000,
    output: 1000,
    cache_read: 1000,
    cache_write: 1000,
    time: Duration::from_secs(3600),
};

const LIMITS: Limits = Limits {
    runs: 1,
    conversations: 1,
    run_bytes: 1024,
    repositories: 1,
    outlets: 1,
    verdicts: 1,
    budget: BUDGET,
    max_tokens: 1024,
    nudges: 1,
};

/// A charter that holds exactly `held` bytes, as the run counts them: one of
/// every part held in a box, at its fixed size plus a byte of payload each,
/// and a brief of the rest.
fn charter(held: u64) -> Charter {
    let label = size(size_of::<Box<[u8]>>());
    let parts = (size(size_of::<Repository>()) + 2) + (size(size_of::<Outlet>()) + 1) + 1;
    let rule = size(size_of::<VerdictRule>()) + 1 + 2 * (label + 1);
    Charter {
        brief: bytes(held - parts - rule),
        checkout: Checkout { repositories: Box::new([Repository { name: bytes(1), path: bytes(1), writable: true }]) },
        grants: Grants {
            tools: Tools { inspect: true, modify: true, shell: true },
            forge: true,
            agents: true,
            outlets: Box::new([Outlet { name: bytes(1) }]),
        },
        outcome: OutcomeSpec {
            change: true,
            verdicts: Box::new([VerdictRule {
                name: bytes(1),
                children: Children { min: 0, max: 1 },
                kinds: Box::new([bytes(1)]),
                fields: Box::new([bytes(1)]),
            }]),
        },
        budget: BUDGET,
        llm: Llm { endpoint: Endpoint(0), model: bytes(1), max_tokens: 1 },
    }
}

/// What a step asked for, without the payload.
#[derive(PartialEq, Eq, Debug)]
enum Asked {
    Open { conversation: Token },
    Answer { answer: Answer },
    Other,
}

/// Fills every run of a model under `limits` with a charter of exactly its
/// byte limit, and has each one's main conversation start, spend, yield and be
/// nudged, checking the heap against the worst case after every step.
fn fill(limits: Limits) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, limits };
    let mut out = Queue::with_capacity(MAX_OUT);
    let base = heap::live();
    let mut model = Model::new(&limits);
    // The requests are the parent's to route and their receivers' to count:
    // each is dropped, keeping only what it asked for, before the heap is
    // measured.
    let mut step = |event: Event| -> Vec<Asked> {
        temper_agent_model_run::step(&mut model, &env, event, &mut out);
        let mut asked = Vec::new();
        while let Some(request) = out.pop() {
            asked.push(match request {
                Request::Open { conversation, .. } => Asked::Open { conversation },
                Request::Answer { answer, to: _ } => Asked::Answer { answer },
                Request::Admitted { .. } | Request::Say { .. } | Request::Close { .. } => Asked::Other,
            });
        }
        let held = held(base);
        assert!(held <= bound, "{limits:?}: the model holds {held} bytes, more than its worst case of {bound}");
        asked
    };
    let spend = Spend { turns: 1, input: 1, output: 1, cache_read: 1, cache_write: 1 };
    for run in 0..limits.runs {
        let worker = Token::new(u64::from(run));
        let start = Event::Start { reply_to: ReplyTo::new(worker), worker, charter: charter(limits.run_bytes) };
        let [Asked::Other, Asked::Open { conversation }] = step(start)[..] else {
            panic!("a charter of exactly the byte limit is admitted");
        };
        assert!(step(Event::Started { conversation, peer: worker }).is_empty(), "starting is quiet");
        assert!(step(Event::Used { conversation, spend }).is_empty(), "within the budget");
        let yielded = Event::Yielded { conversation, stop: Stop::EndTurn, text: bytes(100) };
        assert_eq!(step(yielded), [Asked::Other], "nudged");
    }
    let held = held(base);
    let full = u64::from(limits.runs) * limits.run_bytes;
    assert!(held >= full, "{limits:?}: every run holds its byte limit");

    // A byte more is refused.
    let mut model = Model::new(&Limits { runs: 1, conversations: 1, ..limits });
    let worker = Token::new(0);
    let start = Event::Start { reply_to: ReplyTo::new(worker), worker, charter: charter(limits.run_bytes + 1) };
    temper_agent_model_run::step(&mut model, &env, start, &mut out);
    let Some(Request::Answer { to: _, answer }) = out.pop() else { panic!("expected an answer") };
    assert_eq!(answer, Answer::Refused(Refusal::Invalid(Invalid::TooLarge)));
}

#[test]
fn a_model_with_every_run_full_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { runs: 64, conversations: 64, run_bytes: 65_536, ..LIMITS });
    fill(Limits { runs: 1000, conversations: 1000, run_bytes: 2048, ..LIMITS });
}
