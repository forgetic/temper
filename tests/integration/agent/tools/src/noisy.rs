//! Random worlds: small limits, faults, latencies that race the deadlines,
//! and sessions with random scripts on the fixture's checkout.

use temper_agent_model_tools::{Grants, Limits};
use temper_checkout_fake::Checkout;
use temper_lib::{Duration, Rng, Time};

use crate::calls::{edit, list, read, read_lines, search, shell, write};
use crate::fixture::Fixture;
use crate::{Settings, Span, Step, World};

const PATHS: [&[u8]; 22] = [
    b".git/config",
    b"vendor/lib/lib.rs",
    b"third_party/lib.rs",
    b"third_party/new.rs",
    b"src/lib.rs",
    b"src",
    b".",
    b"Cargo.toml",
    b"big.txt",
    b"long.txt",
    b"link",
    b"srclink/main.rs",
    b"escape",
    b"absolute",
    b"loop",
    b"dev",
    b"missing.rs",
    b"../docs/guide.md",
    b"vendor/lib",
    b"/etc/passwd",
    b"../..",
    b"Cargo.toml/x",
];

const HOT: [&[u8]; 3] = [b"src/lib.rs", b"Cargo.toml", b"src/new.rs"];

/// What commands the LLM runs.
const COMMANDS: [&[u8]; 7] =
    [b"cargo test", b"cargo fmt", b"env", b"sleep 600", b"kill -9 $$", b"vendor update", b"install hooks"];

/// What edits replace: in every file, in some, in one line of one, nowhere.
const SNIPPETS: [&[u8]; 5] = [b"\n", b"pub", b"fn one", b"written", b"nowhere"];

/// A world drawn from `seed`: small limits, faults, latencies that race the
/// deadlines, and up to five sessions with random scripts, some changing the
/// checkout as they go.
#[must_use]
pub fn noisy_world(seed: u64) -> World {
    let mut rng = Rng::new(seed);
    let calm = Settings::calm(seed);
    let mut millis = |low: u64, high: u64| Duration::from_millis(rng.between(low, high));
    let file_timeout = millis(50, 5_000);
    let call_timeout = millis(100, 10_000);
    let io = Span { min: Duration::from_millis(1), max: millis(1, 3_000) };
    let think = Span { min: Duration::from_millis(1), max: millis(1, 500) };
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut pick = |low: u64, high: u64| u32::try_from(rng.between(low, high)).expect("small numbers");
    let settings = Settings {
        tools: Limits {
            kits: pick(1, 3),
            calls: pick(1, 4),
            known_files: pick(1, 4),
            file_bytes: pick(64, 8192),
            read_bytes: pick(16, 2048),
            list_entries: pick(1, 12),
            facts: pick(1, 64),
            file_timeout,
            ..calm.tools
        },
        io,
        faults: pick(0, 200),
        late_cancels: pick(0, 500),
        late_effects: pick(0, 500),
        think,
        call_timeout,
        ..calm
    };
    let fixture = Fixture::new();
    let mut rng = Rng::new(seed.wrapping_add(2));
    let mut authorities = Vec::new();
    for _ in 0..rng.between(1, 5) {
        let grants = Grants { inspect: !rng.chance(100), modify: rng.chance(500), shell: rng.chance(200) };
        authorities.push(fixture.authority(grants));
    }
    let mut world = World::new(settings, fixture.checkout);
    for authority in authorities {
        let at = Time::from_nanos(rng.between(0, 10_000_000_000));
        let script = noisy_script(&mut rng);
        world.session(at, authority, script);
    }
    world
}

fn noisy_script(rng: &mut Rng) -> Vec<Step> {
    let mut script = Vec::new();
    for _ in 0..rng.between(0, 10) {
        let step = match rng.below(13) {
            // As an LLM does: read a file, then edit it.
            12 => {
                let path = HOT[usize::try_from(rng.below(HOT.len() as u64)).expect("an index")];
                script.push(Step::Calls(vec![read(path)]));
                let old = SNIPPETS[usize::try_from(rng.below(SNIPPETS.len() as u64)).expect("an index")];
                Step::Calls(vec![edit(path, old, b"edited", rng.chance(300))])
            }
            0..=4 => Step::Calls(noisy_calls(rng)),
            5..=7 => Step::Send(noisy_calls(rng)),
            8 => Step::Sleep(Duration::from_millis(rng.between(1, 5_000))),
            // A slow spell, or the end of one.
            9 => Step::Latency(Span::millis(1, rng.between(1, 8_000))),
            10 => Step::Change(Box::new(|checkout: &mut Checkout| {
                checkout.write(b"work/temper/src/lib.rs", b"pub fn changed() {}\n");
                checkout.write(b"work/temper/Cargo.toml", b"[package]\n");
            })),
            _ => Step::Change(Box::new(|checkout: &mut Checkout| checkout.remove(b"work/temper/src/main.rs"))),
        };
        script.push(step);
    }
    script
}

fn noisy_calls(rng: &mut Rng) -> Vec<temper_agent_model_tools::Call> {
    let mut calls = Vec::new();
    for _ in 0..rng.between(1, 4) {
        // Half the calls are about a few files, so that kits and changes meet.
        let pool: &[&[u8]] = if rng.chance(500) { &HOT } else { &PATHS };
        let path = pool[usize::try_from(rng.below(pool.len() as u64)).expect("an index")];
        let call = match rng.below(10) {
            9 => search(path, SNIPPETS[usize::try_from(rng.below(SNIPPETS.len() as u64)).expect("an index")], None),
            8 => shell(COMMANDS[usize::try_from(rng.below(COMMANDS.len() as u64)).expect("an index")], None),
            0 => list(path),
            1 => read_lines(path, u32::try_from(rng.below(5)).expect("small"), 2),
            2 | 3 => write(path, format!("written {}\n", rng.below(1000)).as_bytes()),
            4 | 5 => {
                let old = SNIPPETS[usize::try_from(rng.below(SNIPPETS.len() as u64)).expect("an index")];
                let new: &[u8] = if rng.chance(50) { old } else { b"edited" };
                edit(path, old, new, rng.chance(300))
            }
            _ => read(path),
        };
        calls.push(call);
    }
    calls
}
