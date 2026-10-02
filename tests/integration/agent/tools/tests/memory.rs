//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the tools sub-model with every kit holding the
//! longest authority, knowing as many files as it may at the longest paths,
//! and running as many edits, writes and reads as it may; and the model driven
//! at random through every terminal io may give, measured after every step.

use temper_agent_model_tools::{
    Authority, Call, Done, Entry, Event, Exit, Expect, Fault, Grants, Kind, Limits, Model, Name, Op, Part, Path, Repo,
    Request, Var, Version, max_out, worst_case,
};
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};

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

const LIMITS: Limits = Limits {
    kits: 2,
    calls: 3,
    repos: 2,
    path_bytes: 63,
    known_files: 4,
    file_bytes: 256,
    read_bytes: 64,
    list_entries: 4,
    match_lines: 4,
    file_timeout: Duration::from_secs(10),
    env_bytes: 256,
    shell_timeout: Duration::from_secs(60),
    shell_timeout_max: Duration::from_secs(600),
    shell_head: 64,
    shell_tail: 128,
};

const GRANTS: Grants = Grants { inspect: true, modify: true, shell: true };

fn name(bytes: &[u8]) -> Name {
    Name::new(bytes.into()).expect("a test name")
}

/// The longest authority `limits` take: a working directory of as many names
/// as fit, a repository at the root, so that a place's path is as long as an
/// absolute one, and the others at mounts as long as they may be.
fn authority(limits: &Limits) -> Authority {
    let len = usize::try_from(limits.path_bytes).expect("a small limit");
    let names = len.div_ceil(2);
    let cwd: Box<[Name]> = (0..names).map(|_| name(b"a")).collect();
    let mut repos = vec![Repo { mount: Box::new([]), root: Token::new(0), writable: true }];
    for repo in 1..limits.repos {
        let mount = format!("{repo:0>len$}");
        repos.push(Repo { mount: Box::new([name(mount.as_bytes())]), root: Token::new(repo.into()), writable: false });
    }
    // An environment as large as it may be, in variables of a byte each.
    let vars = limits.env_bytes / 4;
    let env =
        (0..vars).map(|var| Var { name: format!("{var:x}").into_bytes().into(), value: b"v"[..].into() }).collect();
    Authority { cwd, repos: repos.into(), grants: GRANTS, env }
}

/// The path of a file whose absolute path is exactly `path_bytes` long,
/// distinct for each `file`.
fn path(limits: &Limits, file: u64) -> Path {
    let len = usize::try_from(limits.path_bytes - 1).expect("a small limit");
    let name = format!("f{file:0>len$}");
    let parts = Box::new([Part::Name { name: Name::new(name.into_bytes().into()).expect("a name") }]);
    Path { absolute: true, parts }
}

fn read(limits: &Limits, file: u64) -> Call {
    Call::Read { path: path(limits, file), skip: 0, lines: None }
}

/// The most a file holds.
fn full(limits: &Limits, byte: u8) -> Box<[u8]> {
    vec![byte; usize::try_from(limits.file_bytes).expect("a small limit")].into()
}

/// A write of a file the kit read, so that it has a version to expect.
fn write(limits: &Limits, file: u64) -> Call {
    Call::Write { path: path(limits, file), content: full(limits, b'x') }
}

/// An edit of a file the kit read, with snippets as long as they may be,
/// which the job holds while it loads the file.
fn edit(limits: &Limits, file: u64) -> Call {
    Call::Edit { path: path(limits, file), old: full(limits, b'x'), new: full(limits, b'y'), all: true }
}

/// Fills every kit of a model under `limits` to its limits, checking the heap
/// against the worst case after every step: it knows twice as many files as
/// it may, the oldest forgotten, then runs as many calls as it may at once,
/// writes of the files it knows and reads.
fn fill(limits: Limits) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let base = heap::live();
    let mut model = Model::new(&limits);
    // Each step is an iteration of its own, ending at the reclaim point. The
    // requests are the session's and io's to hold and count: each is dropped,
    // keeping only the token it names, before the heap is measured.
    let mut step = |event: Event| -> Vec<Token> {
        temper_agent_model_tools::step(&mut model, &env, event, &mut out);
        model.reclaim();
        let mut named = Vec::new();
        while let Some(request) = out.pop() {
            match request {
                Request::Opened { kit, .. } => named.push(kit),
                Request::Io { owner, .. } => named.push(owner),
                Request::Answer { .. } => {}
                request @ (Request::Refused { .. } | Request::Closed { .. } | Request::CancelIo { .. }) => {
                    panic!("{limits:?}: unexpected {request:?}")
                }
            }
        }
        let held = held(base);
        assert!(held <= bound, "{limits:?}: the model holds {held} bytes, more than its worst case of {bound}");
        named
    };
    let deadline = Time::ZERO.saturating_add(Duration::from_secs(60));
    let mut file = 0;
    for session in 0..limits.kits {
        let opened = step(Event::Open { session: Token::new(session.into()), authority: authority(&limits) });
        let [kit] = opened[..] else { panic!("{limits:?}: the authority fits") };
        for _ in 0..limits.known_files * 2 {
            file += 1;
            let reply_to = ReplyTo::new(Token::new(file));
            let [owner] = step(Event::Call { kit, reply_to, call: read(&limits, file), deadline })[..] else {
                panic!("{limits:?}: the read runs");
            };
            let done = Done::Loaded { content: b"x\n"[..].into(), version: Version::new([file, 0, 0, 0]) };
            assert!(step(Event::Done { owner, done }).is_empty(), "a read answers");
        }
        for call in 0..u64::from(limits.calls) {
            let call = if call < u64::from(limits.known_files) {
                if call % 2 == 0 { edit(&limits, file - call) } else { write(&limits, file - call) }
            } else {
                file += 1;
                read(&limits, file)
            };
            let reply_to = ReplyTo::new(Token::new(file + 1000));
            assert_eq!(step(Event::Call { kit, reply_to, call, deadline }).len(), 1, "{limits:?}: the call runs");
        }
    }
    let held = held(base);
    let known = u64::from(limits.kits) * u64::from(limits.known_files) * u64::from(limits.path_bytes);
    assert!(held >= known, "{limits:?}: every kit knows as many files as it may, at the longest paths");
    drop(model);
}

#[test]
fn a_model_with_every_kit_full_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { kits: 16, calls: 8, repos: 8, known_files: 64, path_bytes: 511, ..LIMITS });
    fill(Limits { kits: 64, calls: 1, repos: 1, known_files: 200, path_bytes: 127, ..LIMITS });
}

/// What an operation in flight asked for, as io would remember it.
#[derive(Clone, Copy, Debug)]
enum Asked {
    Load,
    Scan,
    Store { creating: bool },
    Spawn,
}

/// Drives a model under `limits` at random for `rounds` steps, each an
/// iteration of its own: kits open and close, calls of every kind arrive,
/// some past their deadline, and io ends operations in any terminal it may,
/// in any order. The heap is checked against the worst case after every step;
/// once every kit has closed and every operation ended, nothing is left.
fn churn(limits: Limits, seed: u64, rounds: u32) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let mut env = Env { now: Time::ZERO, limits };
    let mut rng = Rng::new(seed);
    // The driver's own containers are allocated before the base, and never
    // grow past their capacity.
    let mut kits: Vec<Token> = Vec::with_capacity(usize::try_from(limits.kits).expect("small") + 1);
    let slots = usize::try_from(limits.kits * limits.calls * 2).expect("small");
    let mut ops: Vec<(Token, Asked)> = Vec::with_capacity(slots);
    let mut out = Queue::with_capacity(max_out(&limits));
    let mut counted = [0; 5];
    let base = heap::live();
    let mut model = Model::new(&limits);
    for round in 0..u64::from(rounds) {
        env.now = Time::from_nanos(round * 1_000_000);
        let event = match rng.below(10) {
            0 => Some(Event::Open { session: Token::new(round), authority: authority(&limits) }),
            1..=4 if !kits.is_empty() => {
                let kit = kits[usize::try_from(rng.below(kits.len() as u64)).expect("an index")];
                let reply_to = ReplyTo::new(Token::new(round));
                let deadline = if rng.chance(50) { env.now } else { env.now.saturating_add(Duration::from_secs(1)) };
                Some(Event::Call { kit, reply_to, call: random_call(&limits, &mut rng), deadline })
            }
            5..=8 if !ops.is_empty() => {
                let (owner, asked) = ops.swap_remove(usize::try_from(rng.below(ops.len() as u64)).expect("an index"));
                Some(Event::Done { owner, done: random_done(&limits, &mut rng, asked) })
            }
            9 if !kits.is_empty() && rng.chance(300) => {
                let kit = kits.swap_remove(usize::try_from(rng.below(kits.len() as u64)).expect("an index"));
                Some(Event::Close { kit })
            }
            _ => None,
        };
        let Some(event) = event else { continue };
        temper_agent_model_tools::step(&mut model, &env, event, &mut out);
        drain(&mut out, &mut kits, &mut ops, &mut counted);
        model.reclaim();
        let held = held(base);
        assert!(held <= bound, "{limits:?}: the model holds {held} bytes, more than its worst case of {bound}");
    }
    // Everything settles: every kit closes, and io ends what is in flight.
    while let Some(kit) = kits.pop() {
        temper_agent_model_tools::step(&mut model, &env, Event::Close { kit }, &mut out);
        drain(&mut out, &mut kits, &mut ops, &mut counted);
    }
    while let Some((owner, _)) = ops.pop() {
        temper_agent_model_tools::step(&mut model, &env, Event::Done { owner, done: Done::Cancelled }, &mut out);
        drain(&mut out, &mut kits, &mut ops, &mut counted);
        model.reclaim();
    }
    model.reclaim();
    assert_eq!((model.kits(), model.jobs()), (0, 0), "{limits:?}: nothing is left");
    assert!(counted.iter().all(|count| *count > 10), "{limits:?}: every kind of operation ran: {counted:?}");
}

/// Takes what a step asked for, keeping what the driver needs to go on, and
/// counting the operations by kind: loads, scans, creates, replaces and
/// spawns.
fn drain(out: &mut Queue<Request>, kits: &mut Vec<Token>, ops: &mut Vec<(Token, Asked)>, seen: &mut [u32; 5]) {
    while let Some(request) = out.pop() {
        match request {
            Request::Opened { kit, .. } => kits.push(kit),
            Request::Io { owner, op, .. } => {
                let (asked, kind) = match op {
                    Op::Load { .. } => (Asked::Load, 0),
                    Op::Scan { .. } => (Asked::Scan, 1),
                    Op::Store { expect: Expect::Absent, .. } => (Asked::Store { creating: true }, 2),
                    Op::Store { expect: Expect::Is { .. }, .. } => (Asked::Store { creating: false }, 3),
                    Op::Spawn { .. } => (Asked::Spawn, 4),
                };
                seen[kind] += 1;
                ops.push((owner, asked));
            }
            Request::Refused { .. } | Request::Answer { .. } | Request::Closed { .. } | Request::CancelIo { .. } => {}
        }
    }
}

fn random_call(limits: &Limits, rng: &mut Rng) -> Call {
    // A few more files than a kit remembers: most writes meet a read, and
    // some meet a file forgotten.
    let file = rng.below(u64::from(limits.known_files) + 2);
    match rng.below(5) {
        0 => read(limits, file),
        1 => Call::List { path: path(limits, file) },
        2 => write(limits, file),
        3 => Call::Shell { command: b"cargo test"[..].into(), timeout: None },
        _ => {
            // Mostly small, so that most edits fit and are stored.
            let len = if rng.chance(100) { u64::from(limits.file_bytes) } else { rng.below(8) };
            let new = vec![b'y'; usize::try_from(len).expect("small")];
            Call::Edit { path: path(limits, file), old: b"x"[..].into(), new: new.into(), all: rng.chance(200) }
        }
    }
}

/// A terminal io may end an operation that asked for `asked` with.
fn random_done(limits: &Limits, rng: &mut Rng, asked: Asked) -> Done {
    let version = Version::new([rng.below(2), 0, 0, 0]);
    match rng.below(6) {
        0 => {
            let common = [Done::Escapes, Done::Failed { fault: Fault::Other }, Done::TimedOut, Done::Cancelled];
            common[usize::try_from(rng.below(4)).expect("an index")].clone()
        }
        _ => match asked {
            Asked::Load => match rng.below(8) {
                0 => Done::Missing,
                1 => Done::TooLarge { size: u64::from(limits.file_bytes) + 1 },
                _ => {
                    let size = usize::try_from(rng.below(u64::from(limits.file_bytes) + 1)).expect("small");
                    let content: Vec<u8> = (0..size).map(|at| if at % 7 == 0 { b'\n' } else { b'x' }).collect();
                    Done::Loaded { content: content.into(), version }
                }
            },
            Asked::Scan => {
                let entries: Vec<Entry> = (0..rng.below(u64::from(limits.list_entries) + 1))
                    .map(|entry| Entry { name: name(format!("entry{entry}").as_bytes()), kind: Kind::File })
                    .collect();
                Done::Scanned { entries: entries.into(), more: rng.below(3) }
            }
            Asked::Spawn => {
                let exit = [Exit::Code { code: 1 }, Exit::Signal { signal: 9 }, Exit::TimedOut];
                let exit = exit[usize::try_from(rng.below(3)).expect("an index")];
                let head = vec![b'h'; usize::try_from(limits.shell_head).expect("small")];
                let tail = vec![b't'; usize::try_from(limits.shell_tail).expect("small")];
                Done::Exited { exit, head: head.into(), tail: tail.into(), dropped: rng.below(1000) }
            }
            Asked::Store { creating } => match rng.below(3) {
                0 if creating => Done::Conflict { now: Some(version) },
                0 => Done::Conflict { now: if rng.chance(500) { Some(version) } else { None } },
                1 => Done::NotFile,
                _ => Done::Stored { version },
            },
        },
    }
}

#[test]
fn a_model_driven_at_random_stays_within_its_worst_case_at_every_step() {
    for seed in 0..20 {
        churn(LIMITS, seed, 2_000);
        churn(Limits { kits: 4, calls: 4, known_files: 8, file_bytes: 64, ..LIMITS }, seed, 2_000);
    }
}
