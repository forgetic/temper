//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the tools child domain with every kit holding the
//! longest authority, knowing as many files as it may at the longest paths,
//! and running as many edits, writes and reads as it may. Driven at random,
//! it is checked in `fuzzy_memory.rs`.

use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use temper_agent_domain_tools::{Domain, Done, Event, Limits, Request, Version, max_out, worst_case};
use temper_agent_tools_world::memory::{LIMITS, authority, edit, read, write};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

/// Fills every kit of a domain under `limits` to its limits, checking the peak
/// of the heap in every step against the worst case: it knows twice as many
/// files as it may, the oldest forgotten, then runs as many calls as it may at
/// once, writes of the files it knows and reads.
fn fill(limits: Limits) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let meter = Meter::new();
    let mut domain = Domain::new(&limits);
    // Each step is an iteration of its own, ending at the reclaim point. The
    // requests are the session's and io's to hold and count: each is dropped,
    // keeping only the token it names, and the step's peak checked less them.
    let mut step = |event: Event| -> Vec<Token> {
        meter.start();
        temper_agent_domain_tools::step(&mut domain, &env, event, &mut out);
        domain.reclaim();
        let measured = meter.end();
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
        meter.check(measured, bound, limits);
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
    let held = meter.held();
    let known = u64::from(limits.kits) * u64::from(limits.known_files) * u64::from(limits.path_bytes);
    assert!(held >= known, "{limits:?}: every kit knows as many files as it may, at the longest paths");
    drop(domain);
}

#[test]
fn a_domain_with_every_kit_full_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { kits: 16, calls: 8, repos: 8, known_files: 64, path_bytes: 511, ..LIMITS });
    fill(Limits { kits: 64, calls: 1, repos: 1, known_files: 200, path_bytes: 127, ..LIMITS });
}
