//! End to end at the tools sub-model: kits opened by sessions the world plays,
//! running their calls on a fake checkout through the io the world plays.

use std::collections::BTreeSet;

use temper_agent_model_tools::{Authority, Entry, Fault, Grants, Kind, Limits, Name, Outcome, Refusal, Repo};
use temper_agent_model_tools_tests::calls::{list, read, read_lines};
use temper_agent_model_tools_tests::{Settings, Span, Step, World, authority, repo};
use temper_checkout_fake::Checkout;
use temper_lib::{Duration, Rng, Time};

const ITERATIONS: u32 = 100_000;

const INSPECT: Grants = Grants { inspect: true, modify: false, shell: false };

const LIB: &[u8] = b"pub fn one() {}\npub fn two() {}\npub fn three() {}\n";

/// A checkout at /work: temper, where relative paths start and which may be
/// written, a library vendored in it, which may not, and the docs beside it;
/// and, outside it all, /etc.
struct Fixture {
    checkout: Checkout,
    repos: Vec<Repo>,
}

impl Fixture {
    fn new() -> Fixture {
        let mut checkout = Checkout::new();
        let repos = vec![
            repo(&mut checkout, b"/work/temper", true),
            repo(&mut checkout, b"/work/temper/vendor/lib", false),
            repo(&mut checkout, b"/work/docs", false),
        ];
        checkout.write(b"work/temper/Cargo.toml", b"[package]\nname = \"temper\"\n");
        checkout.write(b"work/temper/src/lib.rs", LIB);
        checkout.write(b"work/temper/src/main.rs", b"fn main() {}\n");
        checkout.write(b"work/temper/big.txt", &[b'x'; 5000]);
        checkout.write(b"work/temper/long.txt", &long());
        checkout.link(b"work/temper/link", b"src/lib.rs");
        checkout.link(b"work/temper/srclink", b"src");
        checkout.link(b"work/temper/escape", b"../../etc/passwd");
        checkout.link(b"work/temper/absolute", b"/etc/passwd");
        checkout.link(b"work/temper/loop", b"loop");
        checkout.special(b"work/temper/dev");
        checkout.write(b"work/temper/vendor/lib/lib.rs", b"// vendored\n");
        checkout.write(b"work/docs/guide.md", b"# Guide\n");
        checkout.write(b"etc/passwd", b"root:x:0:0\n");
        Fixture { checkout, repos }
    }

    fn authority(&self, grants: Grants) -> Authority {
        authority(b"/work/temper", self.repos.clone(), grants)
    }
}

/// A hundred lines of twenty bytes.
fn long() -> Vec<u8> {
    (0..100).flat_map(|line| format!("line {line:03} ..........\n").into_bytes()).collect()
}

fn read_of(content: &[u8], skipped: u32, lines: u32, total: u32) -> Outcome {
    Outcome::Read { content: content.into(), skipped, lines, total, cut: false }
}

fn entry(name: &[u8], kind: Kind) -> Entry {
    Entry { name: Name::new(name.into()).expect("a name"), kind }
}

/// Runs one session with `grants` and `script` in a world with `settings`,
/// and returns its answers and the world.
fn run(settings: Settings, grants: Grants, script: Vec<Step>) -> (Vec<Outcome>, World) {
    let fixture = Fixture::new();
    let authority = fixture.authority(grants);
    let mut world = World::new(settings, fixture.checkout);
    let session = world.session(Time::ZERO, authority, script);
    world.run(ITERATIONS);
    let answers = world.answers(session).into_iter().cloned().collect();
    (answers, world)
}

#[test]
fn a_kit_reads_and_lists_its_checkout() {
    let script = vec![Step::Calls(vec![
        read(b"src/lib.rs"),
        read_lines(b"src/lib.rs", 1, 1),
        list(b"src"),
        read(b"../docs/guide.md"),
        read(b"vendor/lib/lib.rs"),
        read(b"/work/temper/Cargo.toml"),
    ])];
    let (answers, world) = run(Settings::calm(1), INSPECT, script);
    let src = [entry(b"lib.rs", Kind::File), entry(b"main.rs", Kind::File)];
    let expected = vec![
        read_of(LIB, 0, 3, 3),
        read_of(b"pub fn two() {}\n", 1, 1, 3),
        Outcome::Listed { entries: src.into(), more: 0 },
        read_of(b"# Guide\n", 0, 1, 1),
        read_of(b"// vendored\n", 0, 1, 1),
        read_of(b"[package]\nname = \"temper\"\n", 0, 2, 2),
    ];
    assert_eq!(answers, expected);
    assert_eq!(world.stats().ops, 6);

    // Calls one after another, and a listing of the working directory.
    let script = vec![Step::Calls(vec![list(b".")]), Step::Calls(vec![read(b"link")])];
    let (answers, _) = run(Settings::calm(2), INSPECT, script);
    let entries = [
        entry(b"Cargo.toml", Kind::File),
        entry(b"absolute", Kind::Link),
        entry(b"big.txt", Kind::File),
        entry(b"dev", Kind::Other),
        entry(b"escape", Kind::Link),
        entry(b"link", Kind::Link),
        entry(b"long.txt", Kind::File),
        entry(b"loop", Kind::Link),
        entry(b"src", Kind::Directory),
        entry(b"srclink", Kind::Link),
        entry(b"vendor", Kind::Directory),
    ];
    assert_eq!(answers, vec![Outcome::Listed { entries: entries.into(), more: 0 }, read_of(LIB, 0, 3, 3)]);
}

#[test]
fn paths_that_lead_out_of_the_checkout_are_outside() {
    let script = vec![Step::Calls(vec![
        // Refused by the names, before io.
        read(b"../../etc/passwd"),
        read(b"/etc/passwd"),
        list(b".."),
        read(b"src/../../../etc/passwd"),
        // Refused by io: a link on the way leads out of the repository.
        read(b"escape"),
        read(b"absolute"),
    ])];
    let (answers, world) = run(Settings::calm(3), INSPECT, script);
    assert_eq!(answers, vec![Outcome::Outside; 6]);
    assert_eq!(world.stats().ops, 2, "only the links reached io");

    // Links inside the repository work, and `..` is resolved by the names.
    let script = vec![Step::Calls(vec![
        read(b"link"),
        read(b"srclink/lib.rs"),
        list(b"srclink"),
        read(b"srclink/../Cargo.toml"),
        read(b"vendor/lib/../../Cargo.toml"),
    ])];
    let (answers, _) = run(Settings::calm(4), INSPECT, script);
    let cargo = read_of(b"[package]\nname = \"temper\"\n", 0, 2, 2);
    let src = [entry(b"lib.rs", Kind::File), entry(b"main.rs", Kind::File)];
    let expected = vec![
        read_of(LIB, 0, 3, 3),
        read_of(LIB, 0, 3, 3),
        Outcome::Listed { entries: src.into(), more: 0 },
        cargo.clone(),
        cargo,
    ];
    assert_eq!(answers, expected);
}

#[test]
fn reads_and_listings_are_bounded() {
    let calm = Settings::calm(5);
    let settings = Settings { tools: Limits { list_entries: 4, ..calm.tools }, ..calm };
    let script =
        vec![Step::Calls(vec![read(b"big.txt"), read(b"long.txt"), read_lines(b"long.txt", 95, 10), list(b".")])];
    let (answers, _) = run(settings, INSPECT, script);
    let long = long();
    let first = [
        entry(b"Cargo.toml", Kind::File),
        entry(b"absolute", Kind::Link),
        entry(b"big.txt", Kind::File),
        entry(b"dev", Kind::Other),
    ];
    let expected = vec![
        Outcome::TooLarge { size: 5000 },
        // As many whole lines as fit the 1024 bytes a read answers with.
        read_of(&long[..1020], 0, 51, 100),
        read_of(&long[1900..], 95, 5, 100),
        Outcome::Listed { entries: first.into(), more: 7 },
    ];
    assert_eq!(answers, expected);
}

#[test]
fn a_kit_without_the_inspect_grant_neither_reads_nor_lists() {
    let modify = Grants { inspect: false, modify: true, shell: true };
    let script = vec![Step::Calls(vec![read(b"src/lib.rs"), list(b"src")])];
    let (answers, world) = run(Settings::calm(6), modify, script);
    assert_eq!(answers, vec![Outcome::NotGranted; 2]);
    assert_eq!(world.stats().ops, 0);
}

#[test]
fn what_is_missing_or_not_a_file_is_said_so() {
    let script = vec![Step::Calls(vec![
        read(b"missing.rs"),
        read(b"nowhere/missing.rs"),
        list(b"nowhere"),
        read(b"src"),
        read(b"dev"),
        list(b"Cargo.toml"),
        read(b"Cargo.toml/x"),
        read(b"loop"),
    ])];
    let (answers, _) = run(Settings::calm(7), INSPECT, script);
    let expected = vec![
        Outcome::NotFound,
        Outcome::NotFound,
        Outcome::NotFound,
        Outcome::NotFile,
        Outcome::NotFile,
        Outcome::NotDirectory,
        Outcome::NotDirectory,
        Outcome::Failed { fault: Fault::Other },
    ];
    assert_eq!(answers, expected);
}

#[test]
fn slow_io_times_out_and_faulty_io_says_why() {
    let calm = Settings::calm(8);
    let slow = Settings { io: Span::millis(20_000, 20_000), ..calm };
    let (answers, world) = run(slow, INSPECT, vec![Step::Calls(vec![read(b"src/lib.rs")])]);
    assert_eq!(answers, vec![Outcome::TimedOut]);
    assert_eq!(world.stats().timeouts, 1);
    assert!(world.now() >= Time::ZERO.saturating_add(calm.tools.file_timeout), "the tools' own deadline");

    // A call due sooner than the tools' own limit times out at its own.
    let hurried = Settings { call_timeout: Duration::from_secs(2), ..slow };
    let (answers, world) = run(hurried, INSPECT, vec![Step::Calls(vec![list(b"src")])]);
    assert_eq!(answers, vec![Outcome::TimedOut]);
    assert!(world.now() < Time::ZERO.saturating_add(calm.tools.file_timeout), "the call's deadline");

    let faulty = Settings { faults: 1000, ..calm };
    let (answers, _) = run(faulty, INSPECT, vec![Step::Calls(vec![read(b"src/lib.rs"), list(b".")])]);
    for answer in answers {
        assert!(matches!(answer, Outcome::Failed { .. }), "{answer:?}");
    }
}

#[test]
fn closing_a_kit_cancels_what_it_runs() {
    let calm = Settings::calm(9);
    let slow = Settings { io: Span::millis(5_000, 5_000), ..calm };
    // The session sends its calls and closes at once.
    let script = vec![Step::Send(vec![read(b"src/lib.rs"), list(b"src")])];
    let (answers, world) = run(slow, INSPECT, script);
    assert_eq!(answers, vec![Outcome::Cancelled; 2]);
    assert_eq!(world.stats().cancels, 2);
    assert!(world.now() < Time::ZERO.saturating_add(Duration::from_secs(5)), "nothing waited for io");

    // Cancels that lose their races: the calls are answered as they ran.
    let late = Settings { late_cancels: 1000, ..slow };
    let script = vec![Step::Send(vec![read(b"link")])];
    let (answers, world) = run(late, INSPECT, script);
    assert_eq!(answers, vec![read_of(LIB, 0, 3, 3)]);
    assert_eq!((world.stats().cancels, world.stats().late_cancels), (1, 1));
}

#[test]
fn calls_and_kits_beyond_their_room_are_refused() {
    let calm = Settings::calm(10);
    let settings = Settings { tools: Limits { calls: 2, kits: 1, ..calm.tools }, ..calm };
    let script = vec![Step::Calls(vec![read(b"src/lib.rs"), read(b"link"), read(b"Cargo.toml")])];
    let fixture = Fixture::new();
    let authorities = [fixture.authority(INSPECT), fixture.authority(INSPECT)];
    let mut world = World::new(settings, fixture.checkout);
    let [first, second] = authorities;
    let busy = world.session(Time::ZERO, first, script);
    let refused = world.session(Time::ZERO, second, Vec::new());
    world.run(ITERATIONS);
    let answers: Vec<&Outcome> = world.answers(busy);
    assert_eq!(answers[2], &Outcome::Busy, "a kit runs two calls at once");
    assert_eq!(world.refusal(refused), Some(Refusal::Busy));

    let fixture = Fixture::new();
    let mut world = World::new(Settings { tools: Limits { repos: 2, ..calm.tools }, ..calm }, Checkout::new());
    let invalid = world.session(Time::ZERO, fixture.authority(INSPECT), Vec::new());
    world.run(ITERATIONS);
    assert_eq!(world.refusal(invalid), Some(Refusal::Invalid));
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let replay = |seed| {
        let mut world = noisy_world(seed);
        world.run(ITERATIONS);
        (world.trace().to_vec(), world.stats(), world.now())
    };
    let (trace, stats, end) = replay(11);
    assert!(trace.len() > 20, "the run did something");
    assert_eq!(replay(11), (trace.clone(), stats, end));
    assert_ne!(replay(12).0, trace);
}

/// Hundreds of worlds with random limits, faults, latencies and scripts: each
/// settles, with every call answered once and nothing left alive or in flight
/// (checked by `World::run`), and between them they reach every way a call
/// can end.
#[test]
fn random_worlds_settle_with_every_call_answered() {
    let mut seen = BTreeSet::new();
    for seed in 0..300 {
        let mut world = noisy_world(seed);
        world.run(ITERATIONS);
        for session in world.sessions().collect::<Vec<_>>() {
            if world.refusal(session).is_some() {
                seen.insert("refused");
                continue;
            }
            for answer in world.answers(session) {
                seen.insert(kind(answer));
            }
        }
    }
    let expected = [
        "busy",
        "cancelled",
        "failed",
        "listed",
        "not a directory",
        "not a file",
        "not found",
        "not granted",
        "outside",
        "read",
        "refused",
        "timed out",
        "too large",
    ];
    assert_eq!(seen, expected.into_iter().collect());
}

fn kind(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Read { .. } => "read",
        Outcome::Listed { .. } => "listed",
        Outcome::Written { .. } => "written",
        Outcome::NotGranted => "not granted",
        Outcome::Outside => "outside",
        Outcome::ReadOnly => "read only",
        Outcome::TooLong => "too long",
        Outcome::NotFound => "not found",
        Outcome::NotFile => "not a file",
        Outcome::NotDirectory => "not a directory",
        Outcome::TooLarge { .. } => "too large",
        Outcome::NotRead => "not read",
        Outcome::Stale => "stale",
        Outcome::Failed { .. } => "failed",
        Outcome::TimedOut => "timed out",
        Outcome::Cancelled => "cancelled",
        Outcome::Busy => "busy",
        Outcome::Unsupported => "unsupported",
    }
}

const PATHS: [&[u8]; 18] = [
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

/// A world drawn from `seed`: small limits, faults, latencies that race the
/// deadlines, and up to five sessions with random scripts, some changing the
/// checkout as they go.
fn noisy_world(seed: u64) -> World {
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
            file_timeout,
            ..calm.tools
        },
        io,
        faults: pick(0, 200),
        late_cancels: pick(0, 500),
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
    for _ in 0..rng.between(0, 8) {
        let step = match rng.below(10) {
            0..=4 => Step::Calls(noisy_calls(rng)),
            5 | 6 => Step::Send(noisy_calls(rng)),
            7 => Step::Sleep(Duration::from_millis(rng.between(1, 5_000))),
            8 => Step::Change(Box::new(|checkout: &mut Checkout| {
                checkout.write(b"work/temper/src/lib.rs", b"pub fn changed() {}\n");
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
        let path = PATHS[usize::try_from(rng.below(PATHS.len() as u64)).expect("an index")];
        let call = match rng.below(4) {
            0 => list(path),
            1 => read_lines(path, u32::try_from(rng.below(5)).expect("small"), 2),
            _ => read(path),
        };
        calls.push(call);
    }
    calls
}
