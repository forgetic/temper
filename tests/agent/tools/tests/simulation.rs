//! End to end at the tools child domain: kits opened by sessions the world
//! plays, running their calls on a fake checkout through the io the world
//! plays.

use skein_lib::{Duration, Time};
use temper_agent_domain_tools::{self as tools, Entry, Fault, Grants, Hit, Kind, Limits, Name, Outcome, Refusal};
use temper_agent_tools_world::calls::{edit, list, read, read_lines, search, shell, write};
use temper_agent_tools_world::fixture::{FORMATTED, Fixture, LIB, long, test_log};
use temper_agent_tools_world::{Settings, Span, Step, World, kind, noisy_world};
use temper_fake_checkout::Checkout;

const ITERATIONS: u32 = 100_000;

const INSPECT: Grants = Grants { inspect: true, modify: false, shell: false };

const MODIFY: Grants = Grants { inspect: true, modify: true, shell: false };

const ALL: Grants = Grants { inspect: true, modify: true, shell: true };

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
        entry(b".git", Kind::Directory),
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
        entry(b"third_party", Kind::Link),
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
        entry(b".git", Kind::Directory),
        entry(b"Cargo.toml", Kind::File),
        entry(b"absolute", Kind::Link),
        entry(b"big.txt", Kind::File),
    ];
    let expected = vec![
        Outcome::TooLarge { size: 5000 },
        // As many whole lines as fit the 1024 bytes a read answers with.
        read_of(&long[..1020], 0, 51, 100),
        read_of(&long[1900..], 95, 5, 100),
        Outcome::Listed { entries: first.into(), more: 9 },
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
        assert_eq!(kind(&answer), "failed", "{answer:?}");
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
    let authority = fixture.authority(INSPECT);
    let mut world = World::new(Settings { tools: Limits { repos: 2, ..calm.tools }, ..calm }, fixture.checkout);
    let invalid = world.session(Time::ZERO, authority, Vec::new());
    world.run(ITERATIONS);
    assert_eq!(world.refusal(invalid), Some(Refusal::Invalid));
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let trace = temper_world::assert_replays(11, 12, |seed| {
        let mut world = noisy_world(seed);
        world.run(ITERATIONS);
        (world.trace().to_vec(), (world.stats(), world.now()))
    });
    assert!(trace.len() > 20, "the run did something");
}

fn written(created: bool) -> Outcome {
    Outcome::Written { created }
}

#[test]
fn a_kit_creates_files_and_replaces_those_its_llm_read() {
    let script = vec![
        Step::Calls(vec![write(b"src/new.rs", b"pub fn new() {}\n"), write(b"tests/deep/it.rs", b"#[test]\n")]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![write(b"src/lib.rs", b"pub fn one() {}\n")]),
        // What its LLM wrote, it knows: no read is needed to write it again.
        Step::Calls(vec![write(b"src/lib.rs", b"pub fn uno() {}\n")]),
    ];
    let (answers, world) = run(Settings::calm(20), MODIFY, script);
    let expected = vec![written(true), written(true), read_of(LIB, 0, 3, 3), written(false), written(false)];
    assert_eq!(answers, expected);
    let checkout = world.checkout();
    assert_eq!(checkout.content(b"work/temper/src/new.rs"), Some(&b"pub fn new() {}\n"[..]));
    assert_eq!(checkout.content(b"work/temper/tests/deep/it.rs"), Some(&b"#[test]\n"[..]));
    assert_eq!(checkout.content(b"work/temper/src/lib.rs"), Some(&b"pub fn uno() {}\n"[..]));
}

#[test]
fn a_kit_may_not_change_a_file_its_llm_has_not_read() {
    let script = vec![
        Step::Calls(vec![write(b"src/lib.rs", b"overwritten")]),
        // A listing is not a read.
        Step::Calls(vec![list(b"src")]),
        Step::Calls(vec![write(b"src/main.rs", b"overwritten")]),
        // Nor is reading the file by another path, through a link to it.
        Step::Calls(vec![read(b"link")]),
        Step::Calls(vec![write(b"src/lib.rs", b"overwritten")]),
    ];
    let (answers, world) = run(Settings::calm(21), MODIFY, script);
    let src = [entry(b"lib.rs", Kind::File), entry(b"main.rs", Kind::File)];
    let listed = Outcome::Listed { entries: src.into(), more: 0 };
    let expected = vec![Outcome::NotRead, listed, Outcome::NotRead, read_of(LIB, 0, 3, 3), Outcome::NotRead];
    assert_eq!(answers, expected);
    assert_eq!(world.checkout().files(), Fixture::new().checkout.files(), "nothing changed");
}

#[test]
fn a_change_made_since_the_llm_read_a_file_is_caught() {
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Change(Box::new(|checkout: &mut Checkout| {
            checkout.write(b"work/temper/src/lib.rs", b"theirs\n");
        })),
        Step::Calls(vec![write(b"src/lib.rs", b"ours\n")]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![write(b"src/lib.rs", b"ours\n")]),
        // Removed since it was read: stale, and then created afresh.
        Step::Calls(vec![read(b"src/main.rs")]),
        Step::Change(Box::new(|checkout: &mut Checkout| checkout.remove(b"work/temper/src/main.rs"))),
        Step::Calls(vec![write(b"src/main.rs", b"fn main() {}\n")]),
        Step::Calls(vec![write(b"src/main.rs", b"fn main() {}\n")]),
    ];
    let (answers, world) = run(Settings::calm(22), MODIFY, script);
    let expected = vec![
        read_of(LIB, 0, 3, 3),
        Outcome::Stale,
        read_of(b"theirs\n", 0, 1, 1),
        written(false),
        read_of(b"fn main() {}\n", 0, 1, 1),
        Outcome::Stale,
        written(true),
    ];
    assert_eq!(answers, expected);
    assert_eq!(world.checkout().content(b"work/temper/src/lib.rs"), Some(&b"ours\n"[..]));
}

#[test]
fn kits_on_one_checkout_catch_each_others_changes() {
    let fixture = Fixture::new();
    let authorities = [fixture.authority(MODIFY), fixture.authority(MODIFY), fixture.authority(MODIFY)];
    let mut world = World::new(Settings::calm(23), fixture.checkout);
    let [first, second, third] = authorities;
    let seconds = Duration::from_secs;
    // The first reads the file, and writes it long after the second has.
    let slow = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Sleep(seconds(10)),
        Step::Calls(vec![write(b"src/lib.rs", b"first\n")]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![write(b"src/lib.rs", b"first\n")]),
    ];
    let quick = vec![
        Step::Sleep(seconds(2)),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![write(b"src/lib.rs", b"second\n")]),
        Step::Calls(vec![write(b"src/new.rs", b"second\n")]),
    ];
    // The third creates the file the second did, never having read it.
    let late = vec![Step::Sleep(seconds(5)), Step::Calls(vec![write(b"src/new.rs", b"third\n")])];
    let slow = world.session(Time::ZERO, first, slow);
    let quick = world.session(Time::ZERO, second, quick);
    let late = world.session(Time::ZERO, third, late);
    world.run(ITERATIONS);
    let expected = [read_of(LIB, 0, 3, 3), Outcome::Stale, read_of(b"second\n", 0, 1, 1), written(false)];
    assert_eq!(world.answers(slow), expected.iter().collect::<Vec<_>>());
    let expected = [read_of(LIB, 0, 3, 3), written(false), written(true)];
    assert_eq!(world.answers(quick), expected.iter().collect::<Vec<_>>());
    assert_eq!(world.answers(late), vec![&Outcome::NotRead]);
    let checkout = world.checkout();
    assert_eq!(checkout.content(b"work/temper/src/lib.rs"), Some(&b"first\n"[..]));
    assert_eq!(checkout.content(b"work/temper/src/new.rs"), Some(&b"second\n"[..]));
}

#[test]
fn writes_stay_in_writable_repositories_of_granted_kits() {
    let script = vec![
        Step::Calls(vec![
            write(b"../docs/guide.md", b"x"),
            write(b"vendor/lib/lib.rs", b"x"),
            write(b"/etc/passwd", b"x"),
            write(b"../../etc/passwd", b"x"),
            write(b"big.txt", &[b'x'; 5000]),
            write(b"src", b"x"),
            write(b"dev", b"x"),
            write(b"Cargo.toml/x", b"x"),
            // A repository's .git is not the LLM's to change.
            write(b".git/config", b"[core]\n\tfsmonitor = evil\n"),
            write(b".git/hooks/pre-commit", b"evil"),
            // As a file system that folds case would take it.
            write(b".GIT/config", b"evil"),
            edit(b"src/.Git/HEAD", b"ref", b"evil", false),
            write(b"vendor/lib/.git/config", b"x"),
        ]),
        // Writes follow no link, at the end of the path or on the way: not
        // into a read-only repository mounted inside a writable one, not out
        // of the checkout, not into a file of their own repository, even
        // once read through the link.
        Step::Calls(vec![write(b"third_party/new.rs", b"x"), write(b"escape/x", b"x"), write(b"srclink/new.rs", b"x")]),
        Step::Calls(vec![read(b"third_party/lib.rs"), read(b"link"), read(b"srclink/main.rs")]),
        Step::Calls(vec![
            write(b"third_party/lib.rs", b"x"),
            edit(b"third_party/lib.rs", b"vendored", b"mine", false),
            write(b"link", b"x"),
            edit(b"srclink/main.rs", b"main", b"start", false),
        ]),
    ];
    let (answers, world) = run(Settings::calm(24), MODIFY, script);
    let expected = vec![
        Outcome::ReadOnly,
        Outcome::ReadOnly,
        Outcome::Outside,
        Outcome::Outside,
        Outcome::TooLarge { size: 5000 },
        Outcome::NotFile,
        Outcome::NotFile,
        Outcome::NotDirectory,
        Outcome::Protected,
        Outcome::Protected,
        Outcome::Protected,
        Outcome::Protected,
        Outcome::ReadOnly,
        Outcome::Linked,
        Outcome::Linked,
        Outcome::Linked,
        read_of(b"// vendored\n", 0, 1, 1),
        read_of(LIB, 0, 3, 3),
        read_of(b"fn main() {}\n", 0, 1, 1),
        Outcome::Linked,
        Outcome::Linked,
        Outcome::Linked,
        Outcome::Linked,
    ];
    assert_eq!(answers, expected);
    assert_eq!(world.checkout().files(), Fixture::new().checkout.files(), "nothing changed");

    let script = vec![Step::Calls(vec![write(b"src/new.rs", b"x")])];
    let (answers, world) = run(Settings::calm(25), INSPECT, script);
    assert_eq!((answers, world.stats().ops), (vec![Outcome::NotGranted], 0));
}

#[test]
fn a_kit_that_knows_too_many_files_forgets_the_one_read_longest_ago() {
    let calm = Settings::calm(26);
    let settings = Settings { tools: Limits { known_files: 1, ..calm.tools }, ..calm };
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![read(b"src/main.rs")]),
        Step::Calls(vec![write(b"src/lib.rs", b"x")]),
        Step::Calls(vec![write(b"src/main.rs", b"x")]),
    ];
    let (answers, _) = run(settings, MODIFY, script);
    let expected = vec![read_of(LIB, 0, 3, 3), read_of(b"fn main() {}\n", 0, 1, 1), Outcome::NotRead, written(false)];
    assert_eq!(answers, expected);
}

#[test]
fn a_write_that_timed_out_may_have_happened_and_the_next_one_finds_out() {
    let calm = Settings::calm(27);
    let settings = Settings { late_effects: 1000, ..calm };
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Latency(Span::millis(20_000, 20_000)),
        Step::Calls(vec![write(b"src/lib.rs", b"ours\n")]),
        Step::Latency(calm.io),
        // The LLM cannot know whether its write happened; the check can.
        Step::Calls(vec![write(b"src/lib.rs", b"ours again\n")]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![write(b"src/lib.rs", b"ours again\n")]),
    ];
    let (answers, world) = run(settings, MODIFY, script);
    let expected =
        vec![read_of(LIB, 0, 3, 3), Outcome::TimedOut, Outcome::Stale, read_of(b"ours\n", 0, 1, 1), written(false)];
    assert_eq!(answers, expected);
    assert_eq!(world.stats().late_effects, 1);
    assert_eq!(world.checkout().content(b"work/temper/src/lib.rs"), Some(&b"ours again\n"[..]));
}

#[test]
fn a_kit_edits_a_file_its_llm_read() {
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
        // What its LLM edited, it knows: it may edit it again.
        Step::Calls(vec![edit(b"src/lib.rs", b"pub fn", b"fn", true)]),
        Step::Calls(vec![read(b"src/lib.rs")]),
    ];
    let (answers, world) = run(Settings::calm(30), MODIFY, script);
    let edited = b"fn uno() {}\nfn two() {}\nfn three() {}\n";
    let expected = vec![
        read_of(LIB, 0, 3, 3),
        Outcome::Edited { replaced: 1 },
        Outcome::Edited { replaced: 3 },
        read_of(edited, 0, 3, 3),
    ];
    assert_eq!(answers, expected);
    assert_eq!(world.checkout().content(b"work/temper/src/lib.rs"), Some(&edited[..]));
}

#[test]
fn an_edit_that_would_be_wrong_changes_nothing() {
    let script = vec![
        Step::Calls(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![
            edit(b"src/lib.rs", b"fn four", b"fn cuatro", false),
            edit(b"src/lib.rs", b"pub fn", b"fn", false),
            edit(b"src/lib.rs", b"fn one", b"fn one", false),
            edit(b"src/lib.rs", b"", b"fn", false),
            edit(b"src/lib.rs", b"{}", &[b'x'; 2000], true),
            edit(b"../docs/guide.md", b"Guide", b"Manual", false),
        ]),
    ];
    let (answers, world) = run(Settings::calm(31), MODIFY, script);
    let expected = vec![
        Outcome::NotRead,
        read_of(LIB, 0, 3, 3),
        Outcome::NoMatch,
        Outcome::Ambiguous { count: 3, lines: Box::new([1, 2, 3]) },
        Outcome::Unchanged,
        Outcome::NoMatch,
        Outcome::TooLarge { size: 6044 },
        Outcome::ReadOnly,
    ];
    assert_eq!(answers, expected);
    assert_eq!(world.checkout().files(), Fixture::new().checkout.files(), "nothing changed");
}

#[test]
fn an_edit_catches_a_change_made_before_it_loads_or_before_it_stores() {
    let fixture = Fixture::new();
    let authorities = [fixture.authority(MODIFY), fixture.authority(MODIFY)];
    let mut world = World::new(Settings::calm(32), fixture.checkout);
    let [editor, other] = authorities;
    let seconds = Duration::from_secs;
    // Another kit edits the file between this one's read and its edit; then,
    // with io slow, an outsider changes it between the edit's load (done by
    // 6s) and its store (done by 11s).
    let editor = world.session(
        Time::ZERO,
        editor,
        vec![
            Step::Calls(vec![read(b"src/lib.rs")]),
            Step::Sleep(seconds(3)),
            Step::Calls(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
            Step::Calls(vec![read(b"src/lib.rs")]),
            Step::Latency(Span::millis(5_000, 5_000)),
            Step::Calls(vec![edit(b"src/lib.rs", b"fn two", b"fn dos", false)]),
        ],
    );
    let other = world.session(
        Time::ZERO,
        other,
        vec![
            Step::Sleep(seconds(1)),
            Step::Calls(vec![read(b"src/lib.rs")]),
            Step::Calls(vec![edit(b"src/lib.rs", b"fn three", b"fn tres", false)]),
            Step::Sleep(seconds(6)),
            Step::Change(Box::new(|checkout: &mut Checkout| {
                checkout.write(b"work/temper/src/lib.rs", b"theirs\n");
            })),
        ],
    );
    world.run(ITERATIONS);
    let theirs = b"pub fn one() {}\npub fn two() {}\npub fn tres() {}\n";
    let expected = [read_of(LIB, 0, 3, 3), Outcome::Stale, read_of(theirs, 0, 3, 3), Outcome::Stale];
    assert_eq!(world.answers(editor), expected.iter().collect::<Vec<_>>());
    assert_eq!(world.answers(other), vec![&read_of(LIB, 0, 3, 3), &Outcome::Edited { replaced: 1 }]);
    assert_eq!(world.checkout().content(b"work/temper/src/lib.rs"), Some(&b"theirs\n"[..]));
}

#[test]
fn a_kit_closing_while_an_edit_runs_stores_nothing_it_was_not_told_of() {
    let calm = Settings::calm(33);
    // The load wins its race with the cancel: the edit stores nothing.
    let settings = Settings { late_cancels: 1000, ..calm };
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Latency(Span::millis(5_000, 5_000)),
        Step::Send(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
    ];
    let (answers, world) = run(settings, MODIFY, script);
    assert_eq!(answers, vec![read_of(LIB, 0, 3, 3), Outcome::Cancelled]);
    assert_eq!(world.checkout().content(b"work/temper/src/lib.rs"), Some(LIB));
    assert_eq!(world.stats().late_cancels, 1);
}

#[test]
fn an_edit_that_timed_out_may_have_happened_and_the_next_one_finds_out() {
    let calm = Settings::calm(34);
    // Each phase takes two seconds and the call has three: the load is done
    // in time, the store is abandoned at the call's deadline, and has taken
    // effect all the same.
    let settings = Settings { late_effects: 1000, call_timeout: Duration::from_secs(3), ..calm };
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Latency(Span::millis(2_000, 2_000)),
        Step::Calls(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
        Step::Latency(calm.io),
        // The LLM cannot know whether its edit happened; the next one's load
        // finds out.
        Step::Calls(vec![edit(b"src/lib.rs", b"fn two", b"fn dos", false)]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![edit(b"src/lib.rs", b"fn two", b"fn dos", false)]),
    ];
    let (answers, world) = run(settings, MODIFY, script);
    let uno = b"pub fn uno() {}\npub fn two() {}\npub fn three() {}\n";
    let expected = vec![
        read_of(LIB, 0, 3, 3),
        Outcome::TimedOut,
        Outcome::Stale,
        read_of(uno, 0, 3, 3),
        Outcome::Edited { replaced: 1 },
    ];
    assert_eq!(answers, expected);
    assert_eq!((world.stats().timeouts, world.stats().late_effects), (1, 1));
}

/// What a session does before it closes its kit, with io slow, to leave a call
/// in a given state; and what the call answers, and what `src/lib.rs` (or the
/// new file, for a create) holds after, if the cancel wins and if it loses.
struct Closing {
    state: &'static str,
    script: fn() -> Vec<Step>,
    won: (Outcome, Option<&'static [u8]>),
    lost: (Outcome, Option<&'static [u8]>),
    file: &'static [u8],
}

fn slow() -> Step {
    Step::Latency(Span::millis(2_000, 2_000))
}

#[test]
fn closing_a_kit_settles_a_call_in_every_state() {
    const UNO: &[u8] = b"pub fn uno() {}\npub fn two() {}\npub fn three() {}\n";
    let table = [
        Closing {
            state: "reading",
            script: || vec![slow(), Step::Send(vec![read(b"src/lib.rs")])],
            won: (Outcome::Cancelled, Some(LIB)),
            lost: (read_of(LIB, 0, 3, 3), Some(LIB)),
            file: b"work/temper/src/lib.rs",
        },
        Closing {
            state: "listing",
            script: || vec![slow(), Step::Send(vec![list(b"vendor/lib")])],
            won: (Outcome::Cancelled, Some(LIB)),
            lost: (Outcome::Listed { entries: [entry(b"lib.rs", Kind::File)].into(), more: 0 }, Some(LIB)),
            file: b"work/temper/src/lib.rs",
        },
        Closing {
            state: "creating",
            script: || vec![slow(), Step::Send(vec![write(b"src/new.rs", b"new\n")])],
            won: (Outcome::Cancelled, None),
            lost: (written(true), Some(b"new\n")),
            file: b"work/temper/src/new.rs",
        },
        Closing {
            state: "replacing",
            script: || {
                vec![Step::Calls(vec![read(b"src/lib.rs")]), slow(), Step::Send(vec![write(b"src/lib.rs", b"new\n")])]
            },
            won: (Outcome::Cancelled, Some(LIB)),
            lost: (written(false), Some(b"new\n")),
            file: b"work/temper/src/lib.rs",
        },
        Closing {
            state: "editing, loading",
            script: || {
                let edit = edit(b"src/lib.rs", b"fn one", b"fn uno", false);
                vec![Step::Calls(vec![read(b"src/lib.rs")]), slow(), Step::Send(vec![edit])]
            },
            // A load that won its race stores nothing.
            won: (Outcome::Cancelled, Some(LIB)),
            lost: (Outcome::Cancelled, Some(LIB)),
            file: b"work/temper/src/lib.rs",
        },
        Closing {
            state: "running",
            script: || vec![Step::Send(vec![shell(b"cargo fmt", None)])],
            won: (Outcome::Cancelled, Some(LIB)),
            lost: (exited(tools::Exit::Code { code: 0 }, b"", b"", 0), Some(FORMATTED)),
            file: b"work/temper/src/lib.rs",
        },
        Closing {
            state: "editing, storing",
            script: || {
                let edit = edit(b"src/lib.rs", b"fn one", b"fn uno", false);
                let wait = Step::Sleep(Duration::from_secs(3));
                vec![Step::Calls(vec![read(b"src/lib.rs")]), slow(), Step::Send(vec![edit]), wait]
            },
            won: (Outcome::Cancelled, Some(LIB)),
            lost: (Outcome::Edited { replaced: 1 }, Some(UNO)),
            file: b"work/temper/src/lib.rs",
        },
    ];
    for (seed, case) in (40..).zip(table) {
        for (late_cancels, (answer, content)) in [(0, &case.won), (1000, &case.lost)] {
            let settings = Settings { late_cancels, ..Settings::calm(seed) };
            let (answers, world) = run(settings, ALL, (case.script)());
            assert_eq!(answers.last(), Some(answer), "{}, late cancels {late_cancels}", case.state);
            assert_eq!(world.checkout().content(case.file), *content, "{}, late cancels {late_cancels}", case.state);
            assert_eq!(world.stats().cancels, 1, "{}", case.state);
        }
    }
}

#[test]
fn a_cancel_that_comes_after_its_operation_ended_changes_nothing() {
    // io and the session take as long: the session closes its kit in the
    // iteration its read ends, and the close comes first.
    let settings = Settings { io: Span::millis(100, 100), think: Span::millis(100, 100), ..Settings::calm(50) };
    let (answers, world) = run(settings, INSPECT, vec![Step::Send(vec![read(b"src/lib.rs")])]);
    assert_eq!(answers, vec![read_of(LIB, 0, 3, 3)]);
    assert_eq!((world.stats().cancels, world.stats().stale_cancels), (0, 1));
}

fn exited(exit: tools::Exit, head: &[u8], tail: &[u8], dropped: u64) -> Outcome {
    Outcome::Exited { exit, head: head.into(), tail: tail.into(), dropped }
}

#[test]
fn a_command_runs_and_its_output_is_kept_at_both_ends() {
    let script = vec![Step::Calls(vec![
        shell(b"cargo test", None),
        shell(b"env", None),
        shell(b"nosuch --flag", None),
        shell(b"kill -9 $$", None),
    ])];
    let (answers, _) = run(Settings::calm(60), ALL, script);
    let log = test_log();
    let (head, tail) = (Settings::calm(60).tools.shell_head as usize, Settings::calm(60).tools.shell_tail as usize);
    let dropped = (log.len() - head - tail) as u64;
    let expected = vec![
        exited(tools::Exit::Code { code: 101 }, &log[..head], &log[log.len() - tail..], dropped),
        // The environment is the authority's, and nothing else.
        exited(tools::Exit::Code { code: 0 }, b"PATH=/usr/bin:/bin\nHOME=/home/agent\n", b"", 0),
        exited(tools::Exit::Code { code: 127 }, b"sh: nosuch --flag: not found\n", b"", 0),
        exited(tools::Exit::Signal { signal: 9 }, b"", b"", 0),
    ];
    assert_eq!(answers, expected);
    assert!(log[log.len() - tail..].ends_with(b"test result: FAILED. 99 passed; 1 failed\n"), "the tail tells");
}

#[test]
fn a_command_past_its_deadline_is_killed_with_what_it_wrote() {
    let calm = Settings::calm(61);
    let settings = Settings { tools: Limits { shell_timeout_max: Duration::from_secs(5), ..calm.tools }, ..calm };
    // Asked for an hour, the command gets the tools' most: five seconds of
    // the six hundred it would take, and so none of its output yet.
    let script = vec![Step::Calls(vec![shell(b"sleep 600", Some(Duration::from_secs(3600)))])];
    let (answers, world) = run(settings, ALL, script);
    assert_eq!(answers, vec![exited(tools::Exit::TimedOut, b"", b"", 0)]);
    assert!(world.now() < Time::ZERO.saturating_add(Duration::from_secs(6)), "killed at its deadline");
    assert_eq!(world.checkout().content(b"work/temper/slept"), None, "nor its changes");
    // A command asking for less gets less, and the call's own deadline holds.
    let hurried = Settings { call_timeout: Duration::from_secs(1), ..calm };
    let (answers, world) = run(hurried, ALL, vec![Step::Calls(vec![shell(b"sleep 600", None)])]);
    assert_eq!(answers, vec![exited(tools::Exit::TimedOut, b"", b"", 0)]);
    assert!(world.now() < Time::ZERO.saturating_add(Duration::from_secs(2)), "killed at the call's deadline");
}

#[test]
fn a_command_that_changes_a_file_its_llm_read_makes_the_next_write_stale() {
    let script = vec![
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![shell(b"cargo fmt", None)]),
        Step::Calls(vec![write(b"src/lib.rs", b"mine\n"), edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
        Step::Calls(vec![read(b"src/lib.rs")]),
        Step::Calls(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false)]),
    ];
    let (answers, world) = run(Settings::calm(62), ALL, script);
    let expected = vec![
        read_of(LIB, 0, 3, 3),
        exited(tools::Exit::Code { code: 0 }, b"", b"", 0),
        Outcome::Stale,
        Outcome::Stale,
        read_of(FORMATTED, 0, 5, 5),
        Outcome::Edited { replaced: 1 },
    ];
    assert_eq!(answers, expected);
    let uno = b"pub fn uno() {}\n\npub fn two() {}\n\npub fn three() {}\n";
    assert_eq!(world.checkout().content(b"work/temper/src/lib.rs"), Some(&uno[..]));
}

#[test]
fn a_command_writes_only_the_repositories_its_kit_may() {
    let script = vec![Step::Calls(vec![shell(b"vendor update", None)])];
    let (answers, world) = run(Settings::calm(63), ALL, script);
    assert_eq!(answers, vec![exited(tools::Exit::Code { code: 0 }, b"updated\n", b"", 0)]);
    let checkout = world.checkout();
    assert_eq!(checkout.content(b"work/temper/vendor/lib/lib.rs"), Some(&b"// vendored\n"[..]), "read-only");
    assert_eq!(checkout.content(b"work/temper/src/new.rs"), Some(&b"new\n"[..]));

    let (answers, world) = run(Settings::calm(64), MODIFY, vec![Step::Calls(vec![shell(b"cargo fmt", None)])]);
    assert_eq!((answers, world.stats().ops), (vec![Outcome::NotGranted], 0));
}

fn hit(path: &[u8], line: u32, text: &[u8]) -> Hit {
    Hit { path: path.into(), line, text: text.into() }
}

#[test]
fn a_search_finds_lines_in_path_order_without_following_links() {
    let script = vec![Step::Calls(vec![
        search(b"src", b"fn", None),
        search(b".", b"vendored", None),
        search(b"src/lib.rs", b"two", None),
        search(b"..", b"Guide", Some(b"*.md")),
        // A link to search through is followed; links beneath it are not.
        search(b"srclink", b"main", None),
        search(b"third_party", b"vendored", None),
    ])];
    let (answers, _) = run(Settings::calm(70), INSPECT, script);
    let found = |hits: Vec<Hit>| Outcome::Found { hits: hits.into(), more: 0, timed_out: false };
    let expected = vec![
        found(vec![
            hit(b"lib.rs", 1, b"pub fn one() {}"),
            hit(b"lib.rs", 2, b"pub fn two() {}"),
            hit(b"lib.rs", 3, b"pub fn three() {}"),
            hit(b"main.rs", 1, b"fn main() {}"),
        ]),
        found(vec![hit(b"vendor/lib/lib.rs", 1, b"// vendored")]),
        found(vec![hit(b"", 2, b"pub fn two() {}")]),
        Outcome::Outside,
        found(vec![hit(b"main.rs", 1, b"fn main() {}")]),
        found(vec![hit(b"lib.rs", 1, b"// vendored")]),
    ];
    assert_eq!(answers, expected);
}

#[test]
fn a_search_is_bounded_and_says_when_it_cannot_run() {
    let calm = Settings::calm(71);
    let settings = Settings { tools: Limits { search_hits: 2, search_bytes: 40, ..calm.tools }, ..calm };
    let script = vec![Step::Calls(vec![
        search(b"src", b"fn", None),
        search(b"long.txt", b"line", None),
        search(b"src", b"fn (", None),
        search(b"nowhere", b"fn", None),
        search(b"escape", b"root", None),
        search(b"../docs", b"Guide", None),
    ])];
    let (answers, _) = run(settings, INSPECT, script);
    let stderr = b"rg: regex parse error: unclosed group\n";
    let expected = vec![
        // Two hits, the second cut where forty bytes of paths and text ran
        // out.
        Outcome::Found {
            hits: [hit(b"lib.rs", 1, b"pub fn one() {}"), hit(b"lib.rs", 2, b"pub fn two() ")].into(),
            more: 2,
            timed_out: false,
        },
        Outcome::Found {
            hits: [hit(b"", 1, b"line 000 .........."), hit(b"", 2, b"line 001 ..........")].into(),
            more: 98,
            timed_out: false,
        },
        Outcome::Exited { exit: tools::Exit::Code { code: 2 }, head: stderr[..].into(), tail: [].into(), dropped: 0 },
        Outcome::NotFound,
        Outcome::Outside,
        Outcome::Found { hits: [hit(b"guide.md", 1, b"# Guide")].into(), more: 0, timed_out: false },
    ];
    assert_eq!(answers, expected);

    let modify = Grants { inspect: false, modify: true, shell: true };
    let (answers, world) = run(Settings::calm(72), modify, vec![Step::Calls(vec![search(b".", b"fn", None)])]);
    assert_eq!((answers, world.stats().ops), (vec![Outcome::NotGranted], 0));
}

#[test]
fn a_world_with_no_room_for_facts_runs_as_one_with_room() {
    let run_with = |facts| {
        let calm = Settings::calm(80);
        let settings = Settings { tools: Limits { facts, ..calm.tools }, ..calm };
        let script = vec![
            Step::Calls(vec![read(b"src/lib.rs"), list(b"src"), read(b"/etc/passwd")]),
            Step::Calls(vec![edit(b"src/lib.rs", b"fn one", b"fn uno", false), shell(b"cargo test", None)]),
        ];
        let (answers, world) = run(settings, ALL, script);
        (
            answers,
            world
                .checkout()
                .files()
                .into_iter()
                .map(|(path, content)| (path.to_vec(), content.to_vec()))
                .collect::<Vec<_>>(),
            world.stats().facts_lost,
        )
    };
    let (answers, files, lost) = run_with(64);
    assert_eq!(lost, 0);
    let (lossy_answers, lossy_files, lossy_lost) = run_with(1);
    assert!(lossy_lost > 0, "facts were dropped");
    assert_eq!((lossy_answers, lossy_files), (answers, files), "nothing decided depends on a fact");
}

#[test]
fn a_command_writes_neither_without_modify_nor_in_a_git_directory() {
    let shell_only = Grants { inspect: true, modify: false, shell: true };
    let script = vec![Step::Calls(vec![write(b"src/new.rs", b"x"), shell(b"install hooks", None)])];
    let (answers, world) = run(Settings::calm(90), shell_only, script);
    assert_eq!(answers, vec![Outcome::NotGranted, exited(tools::Exit::Code { code: 0 }, b"", b"", 0)]);
    assert_eq!(world.checkout().files(), Fixture::new().checkout.files(), "a kit without modify writes nothing");

    let (_, world) = run(Settings::calm(91), ALL, vec![Step::Calls(vec![shell(b"install hooks", None)])]);
    let checkout = world.checkout();
    assert_eq!(checkout.content(b"work/temper/src/new.rs"), Some(&b"new\n"[..]), "a kit with modify writes its tree");
    assert_eq!(checkout.content(b"work/temper/.git/hooks/pre-commit"), None, "but never its git directory");
    assert_eq!(checkout.content(b"work/temper/.GIT/config"), None, "in any case");
}

#[test]
fn a_search_past_its_deadline_answers_with_what_it_had_found() {
    let calm = Settings::calm(92);
    let settings = Settings { tools: Limits { search_timeout: Duration::from_secs(10), ..calm.tools }, ..calm };
    let script = vec![Step::Latency(Span::millis(20_000, 20_000)), Step::Calls(vec![search(b"src", b"fn", None)])];
    let (answers, _) = run(settings, INSPECT, script);
    // Half way through the twenty seconds it needed, half the four lines.
    let hits = [hit(b"lib.rs", 1, b"pub fn one() {}"), hit(b"lib.rs", 2, b"pub fn two() {}")];
    assert_eq!(answers, vec![Outcome::Found { hits: hits.into(), more: 0, timed_out: true }]);
}

#[test]
fn a_write_that_failed_may_have_happened_and_the_next_one_finds_out() {
    let calm = Settings::calm(93);
    // io fails the store after it renamed the file into place.
    let script = vec![Step::Calls(vec![write(b"src/new.rs", b"first\n")])];
    let faulty = Settings { faults: 1000, late_effects: 1000, ..calm };
    let (answers, world) = run(faulty, MODIFY, script);
    assert_eq!(answers.len(), 1);
    assert_eq!(kind(&answers[0]), "failed");
    assert_eq!(world.checkout().content(b"work/temper/src/new.rs"), Some(&b"first\n"[..]), "it happened");
    assert_eq!(world.stats().late_effects, 1);

    // The LLM, told it failed, writes again: the file it would create is
    // there, and it has not read it.
    let fixture = Fixture::new();
    let authority = fixture.authority(MODIFY);
    let mut checkout = fixture.checkout;
    checkout.write(b"work/temper/src/new.rs", b"first\n");
    let mut world = World::new(calm, checkout);
    let script = vec![
        Step::Calls(vec![write(b"src/new.rs", b"second\n")]),
        Step::Calls(vec![read(b"src/new.rs")]),
        Step::Calls(vec![write(b"src/new.rs", b"second\n")]),
    ];
    let session = world.session(Time::ZERO, authority, script);
    world.run(ITERATIONS);
    let expected = [Outcome::NotRead, read_of(b"first\n", 0, 1, 1), written(false)];
    assert_eq!(world.answers(session), expected.iter().collect::<Vec<_>>());
}
