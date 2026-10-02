//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the tools sub-model with every kit holding the
//! longest authority, knowing as many files as it may at the longest paths,
//! and running as many reads and writes as it may.

use temper_agent_model_tools::{
    Authority, Call, Done, Event, Grants, Limits, Model, Name, Part, Path, Repo, Request, Version, max_out, worst_case,
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

const LIMITS: Limits = Limits {
    kits: 2,
    calls: 3,
    repos: 2,
    path_bytes: 63,
    known_files: 4,
    file_bytes: 256,
    read_bytes: 64,
    list_entries: 4,
    file_timeout: Duration::from_secs(10),
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
    Authority { cwd, repos: repos.into(), grants: GRANTS }
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

/// A write of a file the kit read, so that it has a version to expect.
fn write(limits: &Limits, file: u64) -> Call {
    let content = vec![b'x'; usize::try_from(limits.file_bytes).expect("a small limit")].into();
    Call::Write { path: path(limits, file), content }
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
                write(&limits, file - call)
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
