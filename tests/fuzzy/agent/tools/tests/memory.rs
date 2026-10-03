//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the tools child domain driven at random through every
//! terminal io may give, its peak measured in every step.

use skein_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token, Wall};
use temper_agent_domain_tools::{
    Call, Domain, Done, Entry, Event, Exit, Expect, Fault, Hit, Kind, Limits, Op, Request, Version, max_out, worst_case,
};
use temper_agent_domain_tools_tests::memory::{LIMITS, authority, name, path, read, write};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

/// What an operation in flight asked for, as io would remember it.
#[derive(Clone, Copy, Debug)]
enum Asked {
    Load,
    Scan,
    Store { creating: bool },
    Spawn,
    Search,
}

/// Drives a domain under `limits` at random for `rounds` steps, each an
/// iteration of its own: kits open and close, calls of every kind arrive,
/// some past their deadline, and io ends operations in any terminal it may,
/// in any order. The peak of the heap in every step is checked against the
/// worst case; once every kit has closed and every operation ended, nothing is
/// left.
fn churn(limits: Limits, seed: u64, rounds: u32) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut rng = Rng::new(seed);
    // The driver's own containers are allocated before the base, and never
    // grow past their capacity.
    let mut kits: Vec<Token> = Vec::with_capacity(usize::try_from(limits.kits).expect("small") + 1);
    let slots = usize::try_from(limits.kits * limits.calls * 2).expect("small");
    let mut ops: Vec<(Token, Asked)> = Vec::with_capacity(slots);
    let mut out = Queue::with_capacity(max_out(&limits));
    let mut counted = [0; 6];
    let meter = Meter::new();
    let mut domain = Domain::new(&limits);
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
        meter.start();
        temper_agent_domain_tools::step(&mut domain, &env, event, &mut out);
        domain.reclaim();
        let measured = meter.end();
        drain(&mut out, &mut kits, &mut ops, &mut counted);
        meter.check(measured, bound, limits);
    }
    // Everything settles: every kit closes, and io ends what is in flight.
    while let Some(kit) = kits.pop() {
        temper_agent_domain_tools::step(&mut domain, &env, Event::Close { kit }, &mut out);
        drain(&mut out, &mut kits, &mut ops, &mut counted);
    }
    while let Some((owner, _)) = ops.pop() {
        temper_agent_domain_tools::step(&mut domain, &env, Event::Done { owner, done: Done::Cancelled }, &mut out);
        drain(&mut out, &mut kits, &mut ops, &mut counted);
        domain.reclaim();
    }
    domain.reclaim();
    assert_eq!((domain.kits(), domain.jobs()), (0, 0), "{limits:?}: nothing is left");
    assert!(counted.iter().all(|count| *count > 10), "{limits:?}: every kind of operation ran: {counted:?}");
}

/// Takes what a step asked for, keeping what the driver needs to go on, and
/// counting the operations by kind: loads, scans, creates, replaces, spawns
/// and searches.
fn drain(out: &mut Queue<Request>, kits: &mut Vec<Token>, ops: &mut Vec<(Token, Asked)>, seen: &mut [u32; 6]) {
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
                    Op::Search { .. } => (Asked::Search, 5),
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
    match rng.below(6) {
        4 => Call::Search { path: path(limits, file), pattern: b"fn"[..].into(), glob: None },
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
            Asked::Search => {
                let hits: Vec<Hit> = (0..rng.below(u64::from(limits.search_hits) + 1))
                    .map(|line| Hit {
                        path: name(b"src").as_bytes().into(),
                        line: u32::try_from(line).expect("small"),
                        text: vec![b'x'; usize::try_from(limits.search_bytes).expect("small")].into(),
                    })
                    .collect();
                Done::Found { hits: hits.into(), more: rng.below(3), timed_out: rng.chance(200) }
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
fn a_domain_driven_at_random_stays_within_its_worst_case_at_every_step() {
    for seed in 0..20 {
        churn(LIMITS, seed, 3_000);
        churn(Limits { kits: 4, calls: 4, known_files: 8, file_bytes: 64, ..LIMITS }, seed, 3_000);
    }
}
