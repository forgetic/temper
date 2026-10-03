//! Feed the domain events, inspect the requests that come out; and the pure
//! functions on paths, by tables.

use alloc::boxed::Box;

use skein_lib::{Duration, Env, Id, List, Queue, ReplyTo, Time, Token, Wall};

use crate::authority::{self, Located};
use crate::edit::{self, Edit};
use crate::kit::Kit;
use crate::knowledge::Knowledge;
use crate::path;
use crate::window::{self, Span};
use crate::{
    Authority, Call, Domain, Done, Effect, Entry, Event, Exit, Expect, Fault, Grants, Hit, Kind, Limits, Name, Op,
    Outcome, Part, Path, Place, Refusal, Repo, Request, Root, Var, Version, effect, max_out, step, worst_case,
};
use crate::{Fact, Tool, Verdict};

const LIMITS: Limits = Limits {
    kits: 2,
    calls: 4,
    repos: 4,
    path_bytes: 64,
    known_files: 8,
    file_bytes: 1024,
    read_bytes: 256,
    list_entries: 16,
    match_lines: 4,
    file_timeout: Duration::from_secs(10),
    env_bytes: 256,
    shell_timeout: Duration::from_secs(60),
    shell_timeout_max: Duration::from_secs(600),
    shell_head: 64,
    shell_tail: 128,
    search_hits: 8,
    search_bytes: 256,
    search_timeout: Duration::from_secs(30),
    facts: 64,
};

const ALL: Grants = Grants { inspect: true, modify: true, shell: true };

/// The domain, its environment, and room for one step's output.
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness {
            domain: Domain::new(&limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(max_out(&limits)),
        }
    }

    fn step(&mut self, event: Event) -> Option<Request> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        let request = self.out.pop();
        assert!(self.out.is_empty(), "one request at most");
        request
    }

    /// Every request one event emits.
    fn emit(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        let max_out = max_out(&self.env.limits);
        let mut requests = List::with_capacity(max_out);
        for _ in 0..max_out {
            if let Some(request) = self.out.pop() {
                requests.push(request).expect("no more than max_out");
            }
        }
        assert!(self.out.is_empty(), "no more than max_out");
        requests.into_boxed()
    }

    /// Sends `call` to `kit`, as call `name`, with a minute to run.
    fn send(&mut self, kit: Token, name: u64, call: Call) -> Option<Request> {
        let reply_to = ReplyTo::new(Token::new(name));
        let deadline = self.env.now.saturating_add(Duration::from_secs(60));
        self.step(Event::Call { kit, reply_to, call, deadline })
    }

    /// Sends `call` to `kit`, which asks io for an operation, and returns it.
    fn start(&mut self, kit: Token, name: u64, call: Call) -> (Token, Op) {
        match self.send(kit, name, call) {
            Some(Request::Io { owner, op, deadline }) => {
                let limit = self.env.now.saturating_add(self.env.limits.file_timeout);
                assert_eq!(deadline, limit, "a file operation has the tools' own deadline, the sooner");
                (owner, op)
            }
            other => panic!("expected an operation, not {other:?}"),
        }
    }

    /// Ends the operation of `owner` with `done`, and returns the answer.
    fn end(&mut self, owner: Token, done: Done) -> (u64, Outcome) {
        match self.step(Event::Done { owner, done }) {
            Some(Request::Answer { to, outcome }) => (to.into_token().raw(), outcome),
            other => panic!("expected an answer, not {other:?}"),
        }
    }

    /// Ends the operation of `owner` with `done`, and returns the operation
    /// its job asks for next.
    fn next(&mut self, owner: Token, done: Done) -> Op {
        match self.step(Event::Done { owner, done }) {
            Some(Request::Io { owner: again, op, .. }) => {
                assert_eq!(again, owner, "a job asks for one operation at a time");
                op
            }
            other => panic!("expected another operation, not {other:?}"),
        }
    }

    fn knowledge(&self, kit: Token) -> &Knowledge {
        let kit: &Kit = self.domain.kits.get(Id::from_token(kit)).expect("the kit is open");
        &kit.knowledge
    }

    /// Opens a kit for `session` with `authority`, and returns its token.
    fn open(&mut self, session: u64, authority: Authority) -> Token {
        let session = Token::new(session);
        match self.step(Event::Open { session, authority }) {
            Some(Request::Opened { session: echoed, kit }) => {
                assert_eq!(echoed, session);
                kit
            }
            other => panic!("expected the kit to open, not {other:?}"),
        }
    }

    /// Runs `call` with `kit`, and returns its answer.
    fn call(&mut self, kit: Token, call: Call) -> Outcome {
        let reply_to = ReplyTo::new(Token::new(99));
        let deadline = Time::ZERO.saturating_add(Duration::from_secs(60));
        match self.step(Event::Call { kit, reply_to, call, deadline }) {
            Some(Request::Answer { to, outcome }) => {
                assert_eq!(to.into_token(), Token::new(99));
                outcome
            }
            other => panic!("expected an answer, not {other:?}"),
        }
    }
}

/// `text` split at its slashes, as the protocol layer would split it.
fn path(text: &[u8]) -> Path {
    let mut parts = List::with_capacity(u32::try_from(text.len()).expect("a short path"));
    let mut start = 0;
    for (index, byte) in text.iter().enumerate() {
        if *byte == b'/' {
            push(&mut parts, &text[start..index]);
            start = index.checked_add(1).expect("within the text");
        }
    }
    push(&mut parts, &text[start..]);
    Path { absolute: text.first() == Some(&b'/'), parts: parts.into_boxed() }
}

fn push(parts: &mut List<Part>, part: &[u8]) {
    let part = match part {
        b"" => return,
        b"." => Part::Current,
        b".." => Part::Parent,
        name => Part::Name { name: Name::new(Box::from(name)).expect("a test name") },
    };
    parts.push(part).expect("room for every part");
}

/// The names of an absolute path.
fn names(text: &[u8]) -> Box<[Name]> {
    let path = path(text);
    assert!(path.absolute, "names are of absolute paths");
    let mut names = List::with_capacity(u32::try_from(path.parts.len()).expect("a short path"));
    for part in path.parts {
        match part {
            Part::Name { name } => names.push(name).expect("room for every name"),
            Part::Current | Part::Parent => panic!("names are normalised"),
        }
    }
    names.into_boxed()
}

/// A checkout at /work, where relative paths start in temper, which may be
/// written; a library vendored in it and the docs beside it may not.
fn authority(grants: Grants) -> Authority {
    Authority {
        cwd: names(b"/work/temper"),
        repos: Box::new([
            Repo { mount: names(b"/work/temper"), root: Token::new(1), writable: true },
            Repo { mount: names(b"/work/temper/vendor/lib"), root: Token::new(2), writable: false },
            Repo { mount: names(b"/work/docs"), root: Token::new(3), writable: false },
        ]),
        grants,
        env: Box::new([var(b"PATH", b"/usr/bin"), var(b"HOME", b"/home/agent")]),
    }
}

fn var(name: &[u8], value: &[u8]) -> Var {
    Var { name: Box::from(name), value: Box::from(value) }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn located(root: u64, path: &[u8], writable: bool) -> Located {
    Located { place: Place { root: Token::new(root), path: Box::from(path) }, writable }
}

fn read(text: &[u8]) -> Call {
    Call::Read { path: path(text), skip: 0, lines: None }
}

fn write(text: &[u8], size: usize) -> Call {
    Call::Write { path: path(text), content: Box::from(&[b'x'; 2048][..size]) }
}

fn edit(text: &[u8]) -> Call {
    Call::Edit { path: path(text), old: bytes(b"a"), new: bytes(b"b"), all: false }
}

/// Opening a kit with `authority` under `limits` is refused as invalid.
fn invalid(limits: Limits, authority: Authority) {
    let mut h = Harness::new(limits);
    let refused = h.step(Event::Open { session: Token::new(1), authority });
    assert_eq!(refused, Some(Request::Refused { session: Token::new(1), refusal: Refusal::Invalid }));
    assert_eq!(h.domain.kits(), 0);
}

#[test]
fn names_are_never_empty_dots_or_slashes() {
    let table: [(&[u8], bool); 9] = [
        (b"lib.rs", true),
        (b".git", true),
        (b"...", true),
        (b"", false),
        (b".", false),
        (b"..", false),
        (b"src/lib.rs", false),
        (b"/", false),
        (b"a\0b", false),
    ];
    for (bytes, valid) in table {
        let made = Name::new(Box::from(bytes));
        assert_eq!(made.is_some(), valid, "{bytes:?}");
        if let Some(made) = made {
            assert_eq!(made.as_bytes(), bytes);
        }
    }
}

#[test]
fn reading_calls_have_the_read_effect_and_the_rest_write() {
    let table = [
        (Call::Read { path: path(b"a"), skip: 0, lines: None }, Effect::Read),
        (Call::List { path: path(b".") }, Effect::Read),
        (Call::Search { path: path(b"."), pattern: bytes(b"fn main"), glob: None }, Effect::Read),
        (Call::Write { path: path(b"a"), content: bytes(b"") }, Effect::Write),
        (Call::Edit { path: path(b"a"), old: bytes(b"x"), new: bytes(b"y"), all: false }, Effect::Write),
        (Call::Shell { command: bytes(b"ls"), timeout: None }, Effect::Write),
    ];
    for (call, expected) in table {
        assert_eq!(effect(&call), expected, "{call:?}");
    }
}

#[test]
fn a_path_normalises_to_the_absolute_names_it_stands_for() {
    let cwd = names(b"/work/temper");
    let table: [(&[u8], Option<&[u8]>); 16] = [
        (b"src/lib.rs", Some(b"work/temper/src/lib.rs")),
        (b"./src/./lib.rs", Some(b"work/temper/src/lib.rs")),
        (b"src//lib.rs/", Some(b"work/temper/src/lib.rs")),
        (b"src/../Cargo.toml", Some(b"work/temper/Cargo.toml")),
        (b"", Some(b"work/temper")),
        (b".", Some(b"work/temper")),
        (b"..", Some(b"work")),
        (b"a/b/../../..", Some(b"work")),
        // `..` at the root stays at the root.
        (b"../../..", Some(b"")),
        (b"../../../etc/passwd", Some(b"etc/passwd")),
        (b"a/../../../../x/..", Some(b"")),
        (b"/etc/passwd", Some(b"etc/passwd")),
        (b"/", Some(b"")),
        (b"/..", Some(b"")),
        (b"/work/temper/../docs/x", Some(b"work/docs/x")),
        // Past `path_bytes` (64) joined.
        (b"0123456789/0123456789/0123456789/0123456789/0123456789", None),
    ];
    for (text, expected) in table {
        let normalised = path::normalise(&cwd, &path(text), LIMITS.path_bytes);
        assert_eq!(normalised.as_deref(), expected, "{text:?}");
    }
    // Exactly at the limit is within it.
    let at_limit = path::normalise(&[], &path(b"/abcd"), 4);
    assert_eq!(at_limit.as_deref(), Some(&b"abcd"[..]));
    assert_eq!(path::normalise(&[], &path(b"/abcde"), 4), None);
}

#[test]
fn a_mount_holds_itself_and_what_is_beneath_it() {
    let table: [(&[u8], &[u8], Option<usize>); 7] = [
        (b"", b"", Some(0)),
        (b"", b"etc/passwd", Some(0)),
        (b"work", b"work", Some(4)),
        (b"work", b"work/temper", Some(5)),
        (b"work", b"workshop", None),
        (b"work/temper", b"work", None),
        (b"work/temper", b"etc", None),
    ];
    for (mount, at, expected) in table {
        assert_eq!(authority::beneath(mount, at), expected, "{mount:?} {at:?}");
    }
}

#[test]
fn a_path_is_in_the_repository_with_the_longest_mount_that_holds_it() {
    let checkout = authority::admit(authority(ALL), &LIMITS).expect("the authority fits");
    let table: [(&[u8], Result<Located, Outcome>); 13] = [
        (b"src/lib.rs", Ok(located(1, b"src/lib.rs", true))),
        (b".", Ok(located(1, b"", true))),
        (b"/work/temper/src", Ok(located(1, b"src", true))),
        (b"vendor/lib/x.rs", Ok(located(2, b"x.rs", false))),
        (b"vendor/lib", Ok(located(2, b"", false))),
        (b"vendor/lib/../lib2", Ok(located(1, b"vendor/lib2", true))),
        (b"vendor/library/x.rs", Ok(located(1, b"vendor/library/x.rs", true))),
        (b"../docs/guide.md", Ok(located(3, b"guide.md", false))),
        (b"..", Err(Outcome::Outside)),
        (b"../temperance/x", Err(Outcome::Outside)),
        (b"/etc/passwd", Err(Outcome::Outside)),
        (b"../../../../etc/passwd", Err(Outcome::Outside)),
        (b"a/very/long/path/that/does/not/fit/in/the/sixty/four/bytes/allowed", Err(Outcome::TooLong)),
    ];
    for (text, expected) in table {
        let located = authority::locate(&checkout, &path(text), LIMITS.path_bytes);
        assert_eq!(located, expected, "{text:?}");
    }
}

#[test]
fn a_repository_mounted_at_the_root_holds_what_no_other_does() {
    let mut authority = authority(ALL);
    authority.repos = Box::new([
        Repo { mount: names(b"/"), root: Token::new(7), writable: false },
        Repo { mount: names(b"/work/temper"), root: Token::new(1), writable: true },
    ]);
    let checkout = authority::admit(authority, &LIMITS).expect("the authority fits");
    let table: [(&[u8], Result<Located, Outcome>); 3] = [
        (b"/etc/passwd", Ok(located(7, b"etc/passwd", false))),
        (b"src", Ok(located(1, b"src", true))),
        (b"/", Ok(located(7, b"", false))),
    ];
    for (text, expected) in table {
        assert_eq!(authority::locate(&checkout, &path(text), LIMITS.path_bytes), expected, "{text:?}");
    }
}

#[test]
fn a_kit_opens_and_closes() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(5, authority(ALL));
    assert_eq!(h.domain.kits(), 1);
    assert_eq!(h.step(Event::Close { kit }), Some(Request::Closed { session: Token::new(5) }));
    assert_eq!(h.domain.kits(), 1, "a closed kit stays until the reclaim point");
    h.domain.reclaim();
    assert_eq!(h.domain.kits(), 0);
}

#[test]
fn opens_beyond_the_kit_slots_are_refused_as_busy() {
    let mut h = Harness::new(Limits { kits: 1, ..LIMITS });
    let _: Token = h.open(1, authority(ALL));
    let refused = h.step(Event::Open { session: Token::new(2), authority: authority(ALL) });
    assert_eq!(refused, Some(Request::Refused { session: Token::new(2), refusal: Refusal::Busy }));
}

#[test]
fn authorities_beyond_the_limits_are_refused_as_invalid() {
    invalid(Limits { repos: 2, ..LIMITS }, authority(ALL));
    // Two repositories at one mount, or with one root.
    let mut twice = authority(ALL);
    twice.repos = Box::new([
        Repo { mount: names(b"/work/temper"), root: Token::new(1), writable: false },
        Repo { mount: names(b"/work/temper"), root: Token::new(2), writable: true },
    ]);
    invalid(LIMITS, twice);
    let mut shared = authority(ALL);
    shared.repos = Box::new([
        Repo { mount: names(b"/work/temper"), root: Token::new(1), writable: true },
        Repo { mount: names(b"/work/docs"), root: Token::new(1), writable: false },
    ]);
    invalid(LIMITS, shared);
    // The vendored library's mount, work/temper/vendor/lib, is 22 bytes.
    invalid(Limits { path_bytes: 21, ..LIMITS }, authority(ALL));
    let mut deep = authority(ALL);
    deep.cwd = names(b"/a/very/deep/working/directory/that/is/longer/than/the/limit/allows");
    invalid(LIMITS, deep);
}

#[test]
fn calls_are_refused_at_the_entrance() {
    let inspect = Grants { inspect: true, modify: false, shell: false };
    let modify = Grants { inspect: false, modify: true, shell: false };
    let shell = Call::Shell { command: bytes(b"cargo test"), timeout: None };
    let table = [
        (modify, read(b"src/lib.rs"), Outcome::NotGranted),
        (modify, Call::List { path: path(b".") }, Outcome::NotGranted),
        (inspect, write(b"src/lib.rs", 1), Outcome::NotGranted),
        (inspect, edit(b"src/lib.rs"), Outcome::NotGranted),
        (inspect, shell.clone(), Outcome::NotGranted),
        (inspect, read(b"/etc/passwd"), Outcome::Outside),
        (inspect, Call::List { path: path(b"..") }, Outcome::Outside),
        (modify, write(b"../../tmp/x", 1), Outcome::Outside),
        (modify, write(b"../docs/guide.md", 1), Outcome::ReadOnly),
        (modify, edit(b"vendor/lib/x.rs"), Outcome::ReadOnly),
        (modify, write(b"src/big.rs", 1025), Outcome::TooLarge { size: 1025 }),
        (modify, write(b".git/config", 1), Outcome::Protected),
        (modify, write(b"src/../.git/hooks/pre-commit", 1), Outcome::Protected),
        (modify, edit(b".git/HEAD"), Outcome::Protected),
        // A repository mounted inside another has its own .git.
        (modify, write(b"vendor/lib/.git/config", 1), Outcome::ReadOnly),
        (inspect, read(b"a/very/long/path/that/does/not/fit/in/the/sixty/four/bytes/allowed"), Outcome::TooLong),
        // What passes the entrance does not run yet.
        (ALL, edit(b"src/lib.rs"), Outcome::NotRead),
    ];
    let mut h = Harness::new(Limits { kits: 32, ..LIMITS });
    for (index, (grants, call, expected)) in table.into_iter().enumerate() {
        let kit = h.open(u64::try_from(index).expect("a small index"), authority(grants));
        assert_eq!(h.call(kit, call.clone()), expected, "{call:?}");
    }
}

#[test]
fn the_most_a_step_emits_covers_closing_a_busy_kit() {
    assert_eq!(max_out(&LIMITS), 4);
    assert_eq!(max_out(&Limits { calls: 1, ..LIMITS }), 2);
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    let paths = u64::from(LIMITS.kits) * u64::from(LIMITS.repos + 1) * u64::from(LIMITS.path_bytes);
    assert!(bytes > paths, "every kit may hold its paths");
    assert_eq!(worst_case(&Limits { kits: u32::MAX, path_bytes: u32::MAX, ..LIMITS }), None);
}

fn place(root: u64, path: &[u8]) -> Place {
    Place { root: Token::new(root), path: Box::from(path) }
}

fn version(n: u64) -> Version {
    Version::new([n, 0, 0, 0])
}

fn entry(name: &[u8], kind: Kind) -> Entry {
    Entry { name: Name::new(Box::from(name)).expect("a test name"), kind }
}

fn span(skip: u32, lines: Option<u32>) -> Span {
    Span { skip, lines }
}

fn read_of(content: &[u8], skipped: u32, lines: u32, total: u32, cut: bool) -> Outcome {
    Outcome::Read { content: Box::from(content), skipped, lines, total, cut }
}

#[test]
fn a_read_answers_with_whole_lines_within_its_limit() {
    let table: [(&[u8], Span, u32, Outcome); 14] = [
        (b"a\nb\nc\n", span(0, None), 100, read_of(b"a\nb\nc\n", 0, 3, 3, false)),
        (b"a\nb\nc", span(0, None), 100, read_of(b"a\nb\nc", 0, 3, 3, false)),
        (b"a\nb\nc\n", span(1, None), 100, read_of(b"b\nc\n", 1, 2, 3, false)),
        (b"a\nb\nc\n", span(1, Some(1)), 100, read_of(b"b\n", 1, 1, 3, false)),
        (b"a\nb\nc\n", span(3, None), 100, read_of(b"", 3, 0, 3, false)),
        (b"a\nb\nc\n", span(9, None), 100, read_of(b"", 3, 0, 3, false)),
        (b"a\nb\nc\n", span(0, Some(0)), 100, read_of(b"", 0, 0, 3, false)),
        (b"aa\nbb\ncc\n", span(0, None), 6, read_of(b"aa\nbb\n", 0, 2, 3, false)),
        (b"aa\nbb\ncc\n", span(0, None), 5, read_of(b"aa\n", 0, 1, 3, false)),
        (b"", span(0, None), 4, read_of(b"", 0, 0, 0, false)),
        (b"\n\n", span(0, None), 4, read_of(b"\n\n", 0, 2, 2, false)),
        // A line longer than a read is cut.
        (b"aaaaaa\nb\n", span(0, None), 4, read_of(b"aaaa", 0, 1, 2, true)),
        (b"aaaaaa", span(0, None), 4, read_of(b"aaaa", 0, 1, 1, true)),
        (b"a\nbbbbbb\n", span(1, Some(5)), 4, read_of(b"bbbb", 1, 1, 2, true)),
    ];
    for (content, span, max, expected) in table {
        let read = window::window(Box::from(content), span, max);
        assert_eq!(read, expected, "{content:?} {span:?} {max}");
    }
}

#[test]
fn a_kit_forgets_the_file_read_longest_ago() {
    let mut knowledge = Knowledge::new(2);
    knowledge.record(place(1, b"a"), version(1));
    knowledge.record(place(1, b"b"), version(2));
    knowledge.record(place(1, b"a"), version(3));
    knowledge.record(place(1, b"c"), version(4));
    assert_eq!(knowledge.version(&place(1, b"a")), Some(version(3)));
    assert_eq!(knowledge.version(&place(1, b"b")), None, "b was read longest ago");
    assert_eq!(knowledge.version(&place(1, b"c")), Some(version(4)));
    assert_eq!(knowledge.version(&place(2, b"c")), None, "places are per repository");
    knowledge.forget(&place(1, b"a"));
    assert_eq!(knowledge.version(&place(1, b"a")), None);

    let mut amnesiac = Knowledge::new(0);
    amnesiac.record(place(1, b"a"), version(1));
    assert_eq!(amnesiac.version(&place(1, b"a")), None);
}

#[test]
fn a_read_loads_the_file_and_answers_with_its_window() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, op) = h.start(kit, 7, Call::Read { path: path(b"src/lib.rs"), skip: 1, lines: Some(1) });
    assert_eq!(op, Op::Load { at: place(1, b"src/lib.rs"), max: LIMITS.file_bytes });
    assert_eq!(h.domain.jobs(), 1);
    let content = Box::from(&b"one\ntwo\nthree\n"[..]);
    let (call, outcome) = h.end(owner, Done::Loaded { content, version: version(5) });
    let expected = Outcome::Read { content: Box::from(&b"two\n"[..]), skipped: 1, lines: 1, total: 3, cut: false };
    assert_eq!((call, outcome), (7, expected));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), Some(version(5)));
    h.domain.reclaim();
    assert_eq!(h.domain.jobs(), 0);
}

#[test]
fn a_read_of_nothing_is_not_found_and_the_kit_knows_it() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, _) = h.start(kit, 1, read(b"src/lib.rs"));
    drop(h.end(owner, Done::Loaded { content: bytes(b""), version: version(5) }));
    let (owner, _) = h.start(kit, 2, read(b"src/lib.rs"));
    assert_eq!(h.end(owner, Done::Missing), (2, Outcome::NotFound));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), None);
}

#[test]
fn every_other_end_of_a_load_answers_for_itself() {
    let table = [
        (Done::NotFile, Outcome::NotFile),
        (Done::NotDirectory, Outcome::NotDirectory),
        (Done::TooLarge { size: 4096 }, Outcome::TooLarge { size: 4096 }),
        (Done::Escapes, Outcome::Outside),
        (Done::Failed { fault: Fault::Denied }, Outcome::Failed { fault: Fault::Denied }),
        (Done::TimedOut, Outcome::TimedOut),
        (Done::Cancelled, Outcome::Cancelled),
    ];
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    for (done, expected) in table {
        let (owner, _) = h.start(kit, 1, read(b"link"));
        assert_eq!(h.end(owner, done), (1, expected));
    }
    assert_eq!(h.knowledge(kit).version(&place(1, b"link")), None);
}

#[test]
fn a_listing_scans_the_directory() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, op) = h.start(kit, 1, Call::List { path: path(b"vendor/lib") });
    assert_eq!(op, Op::Scan { at: place(2, b""), max: LIMITS.list_entries });
    let entries: Box<[Entry]> = Box::new([entry(b"Cargo.toml", Kind::File), entry(b"src", Kind::Directory)]);
    let (_, outcome) = h.end(owner, Done::Scanned { entries: entries.clone(), more: 3 });
    assert_eq!(outcome, Outcome::Listed { entries, more: 3 });

    let table = [
        (Done::Missing, Outcome::NotFound),
        (Done::NotDirectory, Outcome::NotDirectory),
        (Done::Escapes, Outcome::Outside),
        (Done::Failed { fault: Fault::Other }, Outcome::Failed { fault: Fault::Other }),
        (Done::TimedOut, Outcome::TimedOut),
        (Done::Cancelled, Outcome::Cancelled),
    ];
    for (done, expected) in table {
        let (owner, _) = h.start(kit, 1, Call::List { path: path(b"src") });
        assert_eq!(h.end(owner, done), (1, expected));
    }
}

#[test]
fn calls_beyond_a_kits_room_are_busy() {
    let mut h = Harness::new(Limits { calls: 2, ..LIMITS });
    let kit = h.open(1, authority(ALL));
    let (first, _) = h.start(kit, 1, read(b"a"));
    drop(h.start(kit, 2, read(b"b")));
    assert_eq!(h.call(kit, read(b"c")), Outcome::Busy);
    // Another kit has room of its own.
    let other = h.open(2, authority(ALL));
    drop(h.start(other, 3, read(b"c")));
    // An answered call makes room at once.
    drop(h.end(first, Done::Missing));
    drop(h.start(kit, 4, read(b"c")));
}

#[test]
fn a_call_past_its_deadline_times_out_at_the_entrance() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    h.env.now = Time::ZERO.saturating_add(Duration::from_secs(5));
    let reply_to = ReplyTo::new(Token::new(1));
    let call = Event::Call { kit, reply_to, call: read(b"a"), deadline: h.env.now };
    let answer = h.step(call);
    assert_eq!(answer, Some(Request::Answer { to: ReplyTo::new(Token::new(1)), outcome: Outcome::TimedOut }));
    // A call due sooner than the tools' own limit keeps its own deadline.
    let soon = h.env.now.saturating_add(Duration::from_secs(1));
    let call = Event::Call { kit, reply_to: ReplyTo::new(Token::new(2)), call: read(b"a"), deadline: soon };
    match h.step(call) {
        Some(Request::Io { deadline, .. }) => assert_eq!(deadline, soon),
        other => panic!("expected an operation, not {other:?}"),
    }
}

#[test]
fn closing_a_kit_cancels_its_calls_and_ends_with_the_last() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(9, authority(ALL));
    let (first, _) = h.start(kit, 1, read(b"a"));
    let (second, _) = h.start(kit, 2, Call::List { path: path(b".") });
    let cancels = h.emit(Event::Close { kit });
    let mut cancelled = [first, second];
    cancelled.sort_unstable();
    let expected: Box<[Request]> =
        Box::new([Request::CancelIo { owner: cancelled[0] }, Request::CancelIo { owner: cancelled[1] }]);
    assert_eq!(cancels, expected);
    assert_eq!(h.end(first, Done::Cancelled), (1, Outcome::Cancelled));
    // The listing won its race with the cancel.
    let last = h.emit(Event::Done { owner: second, done: Done::Scanned { entries: Box::new([]), more: 0 } });
    let expected: Box<[Request]> = Box::new([
        Request::Answer {
            to: ReplyTo::new(Token::new(2)),
            outcome: Outcome::Listed { entries: Box::new([]), more: 0 },
        },
        Request::Closed { session: Token::new(9) },
    ]);
    assert_eq!(last, expected);
    h.domain.reclaim();
    assert_eq!((h.domain.kits(), h.domain.jobs()), (0, 0));
}

#[test]
fn a_kit_whose_calls_end_and_start_in_one_iteration_has_room() {
    let mut h = Harness::new(Limits { kits: 1, calls: 1, ..LIMITS });
    let kit = h.open(1, authority(ALL));
    let (mut owner, _) = h.start(kit, 1, read(b"a"));
    for name in 2..10 {
        h.domain.reclaim();
        // The call ends, and the next starts before the reclaim point.
        drop(h.end(owner, Done::Missing));
        let (next, _) = h.start(kit, name, read(b"a"));
        assert_eq!(h.domain.jobs(), 2, "the answered job waits for the reclaim point");
        owner = next;
    }
}

fn store(at: Place, content: &[u8], expect: Expect) -> Op {
    Op::Store { at, content: Box::from(content), expect }
}

#[test]
fn a_write_creates_a_file_the_kit_knows_nothing_of() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, op) = h.start(kit, 1, write(b"src/new.rs", 3));
    assert_eq!(op, store(place(1, b"src/new.rs"), b"xxx", Expect::Absent));
    assert_eq!(h.end(owner, Done::Stored { version: version(4) }), (1, Outcome::Written { created: true }));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/new.rs")), Some(version(4)));
    // Its LLM wrote it, so it may write it again.
    let (owner, op) = h.start(kit, 2, write(b"src/new.rs", 1));
    assert_eq!(op, store(place(1, b"src/new.rs"), b"x", Expect::Is { version: version(4) }));
    assert_eq!(h.end(owner, Done::Stored { version: version(5) }), (2, Outcome::Written { created: false }));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/new.rs")), Some(version(5)));
}

#[test]
fn a_write_replaces_a_file_at_the_version_its_llm_read() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, _) = h.start(kit, 1, read(b"src/lib.rs"));
    drop(h.end(owner, Done::Loaded { content: bytes(b"old\n"), version: version(7) }));
    let (owner, op) = h.start(kit, 2, write(b"./src/../src/lib.rs", 2));
    assert_eq!(op, store(place(1, b"src/lib.rs"), b"xx", Expect::Is { version: version(7) }), "places are normalised");
    assert_eq!(h.end(owner, Done::Stored { version: version(8) }), (2, Outcome::Written { created: false }));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), Some(version(8)));
}

#[test]
fn a_write_that_conflicts_with_the_real_file_is_refused() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    // Creating over a file the LLM never read, even one removed before io
    // could tell its version.
    let (owner, _) = h.start(kit, 1, write(b"src/lib.rs", 1));
    assert_eq!(h.end(owner, Done::Conflict { now: Some(version(3)) }), (1, Outcome::NotRead));
    let (owner, _) = h.start(kit, 1, write(b"src/lib.rs", 1));
    assert_eq!(h.end(owner, Done::Conflict { now: None }), (1, Outcome::NotRead));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), None, "the LLM still has not read it");

    // Replacing a file changed since the LLM read it.
    let (owner, _) = h.start(kit, 2, read(b"src/lib.rs"));
    drop(h.end(owner, Done::Loaded { content: bytes(b""), version: version(3) }));
    let (owner, _) = h.start(kit, 3, write(b"src/lib.rs", 1));
    assert_eq!(h.end(owner, Done::Conflict { now: Some(version(4)) }), (3, Outcome::Stale));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), Some(version(3)), "what the LLM read");

    // Replacing a file removed since: the kit knows it is gone, and the next
    // write creates it.
    let (owner, _) = h.start(kit, 4, write(b"src/lib.rs", 1));
    assert_eq!(h.end(owner, Done::Conflict { now: None }), (4, Outcome::Stale));
    let (_, op) = h.start(kit, 5, write(b"src/lib.rs", 1));
    assert_eq!(op, store(place(1, b"src/lib.rs"), b"x", Expect::Absent));
}

#[test]
fn every_other_end_of_a_store_answers_for_itself() {
    let table = [
        (Done::NotFile, Outcome::NotFile),
        (Done::Linked, Outcome::Linked),
        (Done::NotDirectory, Outcome::NotDirectory),
        (Done::Escapes, Outcome::Outside),
        (Done::Failed { fault: Fault::NoSpace }, Outcome::Failed { fault: Fault::NoSpace }),
        (Done::TimedOut, Outcome::TimedOut),
        (Done::Cancelled, Outcome::Cancelled),
    ];
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, _) = h.start(kit, 1, read(b"link"));
    drop(h.end(owner, Done::Loaded { content: bytes(b""), version: version(2) }));
    for (done, expected) in table {
        let (owner, op) = h.start(kit, 1, write(b"link", 1));
        assert_eq!(op, store(place(1, b"link"), b"x", Expect::Is { version: version(2) }));
        assert_eq!(h.end(owner, done), (1, expected));
        // Whether or not the store happened, the next one's check settles it.
        assert_eq!(h.knowledge(kit).version(&place(1, b"link")), Some(version(2)));
    }
}

#[test]
fn a_kit_that_forgot_a_file_must_read_it_again_to_change_it() {
    let mut h = Harness::new(Limits { known_files: 1, ..LIMITS });
    let kit = h.open(1, authority(ALL));
    for (name, file) in [(1, &b"a"[..]), (2, b"b")] {
        let (owner, _) = h.start(kit, name, read(file));
        drop(h.end(owner, Done::Loaded { content: bytes(b""), version: version(name) }));
    }
    let (_, op) = h.start(kit, 3, write(b"a", 1));
    assert_eq!(op, store(place(1, b"a"), b"x", Expect::Absent), "a was forgotten for b");
}

fn snippet(old: &[u8], new: &[u8], all: bool) -> Edit {
    Edit { old: Box::from(old), new: Box::from(new), all }
}

fn edited(content: &[u8], replaced: u32) -> (Box<[u8]>, u32) {
    (Box::from(content), replaced)
}

/// What an edit makes of a file: its new content and how many occurrences it
/// replaced, or the outcome that refuses it.
type Applied = Result<(Box<[u8]>, u32), Outcome>;

fn ambiguous(count: u32, lines: &[u32]) -> Applied {
    Err(Outcome::Ambiguous { count, lines: Box::from(lines) })
}

#[test]
fn an_edit_replaces_its_one_match_or_all_of_them() {
    let small = Limits { file_bytes: 8, match_lines: 2, ..LIMITS };
    let table: [(&[u8], Edit, &Limits, Applied); 10] = [
        (b"a b a", snippet(b"b", b"c", false), &LIMITS, Ok(edited(b"a c a", 1))),
        (b"a b a", snippet(b"a", b"c", false), &LIMITS, ambiguous(2, &[1, 1])),
        (b"x\na\ny\na\na\n", snippet(b"a", b"b", false), &LIMITS, ambiguous(3, &[2, 4, 5])),
        (b"x\na\ny\na\na\n", snippet(b"a", b"b", false), &small, ambiguous(3, &[2, 4])),
        (b"x\na\ny\na\na\n", snippet(b"a", b"bb", true), &LIMITS, Ok(edited(b"x\nbb\ny\nbb\nbb\n", 3))),
        (b"a b a", snippet(b"z", b"c", true), &LIMITS, Err(Outcome::NoMatch)),
        // Matches do not overlap.
        (b"aaa", snippet(b"aa", b"b", false), &LIMITS, Ok(edited(b"ba", 1))),
        (b"abc", snippet(b"abc", b"", false), &LIMITS, Ok(edited(b"", 1))),
        (b"abc", snippet(b"b", b"0123456789", false), &small, Err(Outcome::TooLarge { size: 12 })),
        (b"abc", snippet(b"b", b"012345", false), &small, Ok(edited(b"a012345c", 1))),
    ];
    for (content, edit, limits, expected) in table {
        assert_eq!(edit::apply(content, &edit, limits), expected, "{content:?} {edit:?}");
    }
}

/// Reads `file` with `kit`, which finds it holding `content` at `version`.
fn known(h: &mut Harness, kit: Token, file: &[u8], content: &[u8], at: u64) {
    let (owner, _) = h.start(kit, 1, read(file));
    drop(h.end(owner, Done::Loaded { content: bytes(content), version: version(at) }));
}

fn change(text: &[u8], old: &[u8], new: &[u8]) -> Call {
    Call::Edit { path: path(text), old: bytes(old), new: bytes(new), all: false }
}

#[test]
fn an_edit_loads_the_file_its_llm_read_and_stores_it_edited() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    known(&mut h, kit, b"src/lib.rs", b"fn one() {}\n", 3);
    let (owner, op) = h.start(kit, 2, change(b"src/lib.rs", b"one", b"uno"));
    assert_eq!(op, Op::Load { at: place(1, b"src/lib.rs"), max: LIMITS.file_bytes });
    let op = h.next(owner, Done::Loaded { content: bytes(b"fn one() {}\n"), version: version(3) });
    assert_eq!(op, store(place(1, b"src/lib.rs"), b"fn uno() {}\n", Expect::Is { version: version(3) }));
    assert_eq!(h.end(owner, Done::Stored { version: version(4) }), (2, Outcome::Edited { replaced: 1 }));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), Some(version(4)));
}

#[test]
fn an_edit_of_a_file_changed_since_it_was_read_is_stale() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    known(&mut h, kit, b"src/lib.rs", b"fn one() {}\n", 3);
    // Changed before the load: nothing is stored.
    let (owner, _) = h.start(kit, 2, change(b"src/lib.rs", b"one", b"uno"));
    let loaded = Done::Loaded { content: bytes(b"fn two() {}\n"), version: version(5) };
    assert_eq!(h.end(owner, loaded), (2, Outcome::Stale));
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), Some(version(3)), "what the LLM read");
    // Changed between the load and the store.
    let (owner, _) = h.start(kit, 3, change(b"src/lib.rs", b"one", b"uno"));
    let loaded = Done::Loaded { content: bytes(b"fn one() {}\n"), version: version(3) };
    drop(h.next(owner, loaded));
    assert_eq!(h.end(owner, Done::Conflict { now: Some(version(6)) }), (3, Outcome::Stale));
    // Removed since: not found, and forgotten.
    let (owner, _) = h.start(kit, 4, change(b"src/lib.rs", b"one", b"uno"));
    assert_eq!(h.end(owner, Done::Missing), (4, Outcome::NotFound));
    assert_eq!(h.call(kit, change(b"src/lib.rs", b"one", b"uno")), Outcome::NotRead);
}

#[test]
fn an_edit_that_matches_nothing_or_too_much_stores_nothing() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    known(&mut h, kit, b"src/lib.rs", b"a\nb\na\n", 3);
    let table = [
        (change(b"src/lib.rs", b"zzz", b"y"), Outcome::NoMatch),
        (change(b"src/lib.rs", b"a", b"y"), Outcome::Ambiguous { count: 2, lines: Box::new([1, 3]) }),
    ];
    for (call, expected) in table {
        let (owner, _) = h.start(kit, 2, call);
        let loaded = Done::Loaded { content: bytes(b"a\nb\na\n"), version: version(3) };
        assert_eq!(h.end(owner, loaded), (2, expected));
    }
    let all = Call::Edit { path: path(b"src/lib.rs"), old: bytes(b"a"), new: bytes(b"y"), all: true };
    let (owner, _) = h.start(kit, 3, all);
    let loaded = Done::Loaded { content: bytes(b"a\nb\na\n"), version: version(3) };
    match h.next(owner, loaded) {
        Op::Store { content, .. } => assert_eq!(&*content, b"y\nb\ny\n"),
        op @ (Op::Load { .. } | Op::Scan { .. } | Op::Spawn { .. } | Op::Search { .. }) => {
            panic!("expected the store, not {op:?}")
        }
    }
}

#[test]
fn edits_are_refused_at_the_entrance_without_a_read_or_a_change() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    assert_eq!(h.call(kit, change(b"src/lib.rs", b"a", b"b")), Outcome::NotRead);
    known(&mut h, kit, b"src/lib.rs", b"a\n", 3);
    let big = Call::Edit { path: path(b"src/lib.rs"), old: bytes(b"a"), new: Box::from(&[b'x'; 1025][..]), all: false };
    let table = [
        (change(b"src/lib.rs", b"", b"b"), Outcome::NoMatch),
        (change(b"src/lib.rs", b"a", b"a"), Outcome::Unchanged),
        (big, Outcome::TooLarge { size: 1025 }),
        (change(b"vendor/lib/x.rs", b"a", b"b"), Outcome::ReadOnly),
    ];
    h.domain.reclaim();
    for (call, expected) in table {
        assert_eq!(h.call(kit, call.clone()), expected, "{call:?}");
    }
    assert_eq!(h.domain.jobs(), 0, "nothing ran");
}

#[test]
fn a_kit_closing_while_an_edit_loads_stores_nothing() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(9, authority(ALL));
    known(&mut h, kit, b"src/lib.rs", b"a\n", 3);
    let (owner, _) = h.start(kit, 2, change(b"src/lib.rs", b"a", b"b"));
    assert_eq!(&*h.emit(Event::Close { kit }), &[Request::CancelIo { owner }]);
    // The load won its race with the cancel; the edit is cancelled all the same.
    let loaded = Done::Loaded { content: bytes(b"a\n"), version: version(3) };
    let expected = [
        Request::Answer { to: ReplyTo::new(Token::new(2)), outcome: Outcome::Cancelled },
        Request::Closed { session: Token::new(9) },
    ];
    assert_eq!(&*h.emit(Event::Done { owner, done: loaded }), &expected);
}

#[test]
fn a_kit_closing_while_an_edit_stores_answers_with_whichever_end_came() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(9, authority(ALL));
    known(&mut h, kit, b"src/lib.rs", b"a\n", 3);
    let (owner, _) = h.start(kit, 2, change(b"src/lib.rs", b"a", b"b"));
    let loaded = Done::Loaded { content: bytes(b"a\n"), version: version(3) };
    drop(h.next(owner, loaded));
    assert_eq!(&*h.emit(Event::Close { kit }), &[Request::CancelIo { owner }]);
    // The store won its race: it happened, and the kit knows it.
    let ended = h.emit(Event::Done { owner, done: Done::Stored { version: version(4) } });
    let expected = [
        Request::Answer { to: ReplyTo::new(Token::new(2)), outcome: Outcome::Edited { replaced: 1 } },
        Request::Closed { session: Token::new(9) },
    ];
    assert_eq!(&*ended, &expected);
    assert_eq!(h.knowledge(kit).version(&place(1, b"src/lib.rs")), Some(version(4)));
}

#[test]
fn an_edit_whose_deadline_passed_while_it_loaded_stores_nothing() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    known(&mut h, kit, b"src/lib.rs", b"a\n", 3);
    let (owner, _) = h.start(kit, 2, change(b"src/lib.rs", b"a", b"b"));
    h.env.now = Time::ZERO.saturating_add(Duration::from_secs(60));
    let loaded = Done::Loaded { content: bytes(b"a\n"), version: version(3) };
    assert_eq!(h.end(owner, loaded), (2, Outcome::TimedOut));
}

#[test]
fn a_path_is_in_git_when_one_of_its_names_is_dot_git() {
    let table: [(&[u8], bool); 13] = [
        (b".git", true),
        (b".GIT/config", true),
        (b"src/.Git", true),
        (b"src/.gitx/.gIt/HEAD", true),
        (b".git/config", true),
        (b"src/.git", true),
        (b"vendor/lib/.git/hooks/post-checkout", true),
        (b"", false),
        (b"src/lib.rs", false),
        (b".github/workflows/ci.yml", false),
        (b"x.git", false),
        (b"src/.gitignore", false),
        (b"a.git/.gitx", false),
    ];
    for (path, expected) in table {
        assert_eq!(path::in_git(path), expected, "{path:?}");
    }
}

/// Sends a command to `kit` with an hour to run, and returns the spawn it asks
/// for and its deadline.
fn spawned(h: &mut Harness, kit: Token, timeout: Option<Duration>) -> (Token, Op, Time) {
    let reply_to = ReplyTo::new(Token::new(1));
    let call = Call::Shell { command: bytes(b"cargo test"), timeout };
    let deadline = h.env.now.saturating_add(Duration::from_secs(3600));
    match h.step(Event::Call { kit, reply_to, call, deadline }) {
        Some(Request::Io { owner, op, deadline }) => (owner, op, deadline),
        other => panic!("expected a spawn, not {other:?}"),
    }
}

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

#[test]
fn a_command_runs_in_the_working_directory_with_the_kits_environment_and_roots() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let (owner, op, deadline) = spawned(&mut h, kit, None);
    let roots: Box<[Root]> = Box::new([
        Root { root: Token::new(1), writable: true },
        Root { root: Token::new(2), writable: false },
        Root { root: Token::new(3), writable: false },
    ]);
    let expected = Op::Spawn {
        cwd: place(1, b""),
        command: bytes(b"cargo test"),
        env: authority(ALL).env,
        roots,
        head: LIMITS.shell_head,
        tail: LIMITS.shell_tail,
    };
    assert_eq!((op, deadline), (expected, at(60)), "the tools' default limit for a command");
    let exited =
        Done::Exited { exit: Exit::Code { code: 101 }, head: bytes(b"running"), tail: bytes(b"FAILED"), dropped: 7 };
    let outcome =
        Outcome::Exited { exit: Exit::Code { code: 101 }, head: bytes(b"running"), tail: bytes(b"FAILED"), dropped: 7 };
    assert_eq!(h.end(owner, exited), (1, outcome));
}

#[test]
fn a_command_runs_as_long_as_it_asks_within_the_tools_limit() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    for (asked, deadline) in [(Some(Duration::from_secs(5)), at(5)), (Some(Duration::from_secs(7200)), at(600))] {
        let (owner, _, given) = spawned(&mut h, kit, asked);
        assert_eq!(given, deadline, "{asked:?}");
        drop(h.end(owner, Done::Cancelled));
    }
    // The call's own deadline holds over both.
    let reply_to = ReplyTo::new(Token::new(1));
    let call = Call::Shell { command: bytes(b"sleep 9"), timeout: Some(Duration::from_secs(5)) };
    match h.step(Event::Call { kit, reply_to, call, deadline: at(2) }) {
        Some(Request::Io { deadline, .. }) => assert_eq!(deadline, at(2)),
        other => panic!("expected a spawn, not {other:?}"),
    }
}

#[test]
fn every_end_of_a_command_answers_for_itself() {
    let table = [
        (
            Done::Exited { exit: Exit::TimedOut, head: bytes(b"h"), tail: bytes(b""), dropped: 0 },
            Outcome::Exited { exit: Exit::TimedOut, head: bytes(b"h"), tail: bytes(b""), dropped: 0 },
        ),
        (Done::Missing, Outcome::NotFound),
        (Done::NotDirectory, Outcome::NotDirectory),
        (Done::Escapes, Outcome::Outside),
        (Done::Failed { fault: Fault::Other }, Outcome::Failed { fault: Fault::Other }),
        (Done::TimedOut, Outcome::TimedOut),
        (Done::Cancelled, Outcome::Cancelled),
    ];
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    for (done, expected) in table {
        let (owner, _, _) = spawned(&mut h, kit, None);
        assert_eq!(h.end(owner, done), (1, expected));
    }
}

#[test]
fn commands_need_the_shell_grant_and_a_working_directory_in_the_checkout() {
    let mut h = Harness::new(LIMITS);
    let modify = h.open(1, authority(Grants { inspect: true, modify: true, shell: false }));
    assert_eq!(h.call(modify, Call::Shell { command: bytes(b"ls"), timeout: None }), Outcome::NotGranted);
    let mut outside = authority(ALL);
    outside.cwd = names(b"/work");
    let outside = h.open(2, outside);
    assert_eq!(h.call(outside, Call::Shell { command: bytes(b"ls"), timeout: None }), Outcome::Outside);
}

#[test]
fn an_environment_that_cannot_be_one_or_is_too_large_is_refused() {
    let table = [var(b"", b"x"), var(b"A=B", b"x"), var(b"A\0", b"x"), var(b"A", b"x\0"), var(b"BIG", &[b'x'; 256])];
    for var in table {
        let mut authority = authority(ALL);
        authority.env = Box::new([var.clone()]);
        invalid(LIMITS, authority);
    }
    // Exactly at the limit is within it: a name, its `=` and its value.
    let mut fits = authority(ALL);
    fits.env = Box::new([var(b"A", &[b'x'; 254])]);
    let mut h = Harness::new(LIMITS);
    let _: Token = h.open(1, fits);
}

#[test]
fn a_search_asks_io_for_bounded_hits_and_answers_with_them() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(ALL));
    let reply_to = ReplyTo::new(Token::new(1));
    let call = Call::Search { path: path(b"src"), pattern: bytes(b"fn"), glob: Some(bytes(b"*.rs")) };
    let (owner, op, deadline) = match h.step(Event::Call { kit, reply_to, call, deadline: at(3600) }) {
        Some(Request::Io { owner, op, deadline }) => (owner, op, deadline),
        other => panic!("expected a search, not {other:?}"),
    };
    let expected = Op::Search {
        at: place(1, b"src"),
        pattern: bytes(b"fn"),
        glob: Some(bytes(b"*.rs")),
        hits: LIMITS.search_hits,
        bytes: LIMITS.search_bytes,
    };
    assert_eq!((op, deadline), (expected, at(30)), "the tools' limit for a search");
    let hits: Box<[Hit]> = Box::new([Hit { path: bytes(b"lib.rs"), line: 3, text: bytes(b"pub fn three() {}") }]);
    let (_, outcome) = h.end(owner, Done::Found { hits: hits.clone(), more: 4, timed_out: true });
    assert_eq!(outcome, Outcome::Found { hits, more: 4, timed_out: true });

    let unreadable =
        Done::Exited { exit: Exit::Code { code: 2 }, head: bytes(b"regex parse error"), tail: bytes(b""), dropped: 0 };
    let table = [
        (
            unreadable.clone(),
            Outcome::Exited {
                exit: Exit::Code { code: 2 },
                head: bytes(b"regex parse error"),
                tail: bytes(b""),
                dropped: 0,
            },
        ),
        (Done::Missing, Outcome::NotFound),
        (Done::NotDirectory, Outcome::NotDirectory),
        (Done::Escapes, Outcome::Outside),
        (Done::TimedOut, Outcome::TimedOut),
        (Done::Cancelled, Outcome::Cancelled),
    ];
    for (done, expected) in table {
        let call = Call::Search { path: path(b"."), pattern: bytes(b"fn ("), glob: None };
        let (owner, _) = match h.send(kit, 2, call) {
            Some(Request::Io { owner, op, .. }) => (owner, op),
            other => panic!("expected a search, not {other:?}"),
        };
        assert_eq!(h.end(owner, done), (2, expected));
    }
    let outside = Call::Search { path: path(b"/etc"), pattern: bytes(b"root"), glob: None };
    assert_eq!(h.call(kit, outside), Outcome::Outside);
    // No NUL can go in an argument.
    let table = [
        Call::Search { path: path(b"."), pattern: bytes(b"a\0b"), glob: None },
        Call::Search { path: path(b"."), pattern: bytes(b"a"), glob: Some(bytes(b"*.rs\0")) },
        Call::Shell { command: bytes(b"ls\0-la"), timeout: None },
    ];
    for call in table {
        assert_eq!(h.call(kit, call.clone()), Outcome::NulByte, "{call:?}");
    }
}

/// Drains the facts told so far.
fn told(h: &mut Harness) -> List<Fact> {
    let mut facts = List::with_capacity(LIMITS.facts);
    for _ in 0..LIMITS.facts {
        if let Some(fact) = h.domain.pop_fact() {
            facts.push(fact).expect("room for every fact");
        }
    }
    facts
}

#[test]
fn a_kit_tells_what_happens_as_facts() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(5, authority(ALL));
    let session = Token::new(5);
    assert_eq!(h.call(kit, read(b"/etc/passwd")), Outcome::Outside);
    let (owner, _) = h.start(kit, 1, read(b"src/lib.rs"));
    drop(h.end(owner, Done::Loaded { content: bytes(b"one\ntwo\n"), version: version(1) }));
    let (_, _) = h.start(kit, 2, Call::List { path: path(b"src") });
    drop(h.emit(Event::Close { kit }));
    let expected = [
        Fact::Opened { session },
        // Refused at the entrance: answered without having started.
        Fact::Answered { session, tool: Tool::Read, verdict: Verdict::Outside, bytes: 0 },
        Fact::Started { session, tool: Tool::Read },
        Fact::Answered { session, tool: Tool::Read, verdict: Verdict::Read, bytes: 8 },
        Fact::Started { session, tool: Tool::List },
        Fact::Closing { session, running: 1 },
    ];
    assert_eq!(told(&mut h).as_slice(), &expected);
    let mut bad = authority(ALL);
    bad.env = Box::new([var(b"A=B", b"")]);
    drop(h.step(Event::Open { session: Token::new(6), authority: bad }));
    let refused = Fact::Refused { session: Token::new(6), refusal: Refusal::Invalid };
    assert_eq!(told(&mut h).as_slice(), &[refused]);
}

#[test]
fn facts_that_do_not_fit_are_counted_and_change_nothing() {
    let mut h = Harness::new(Limits { facts: 1, ..LIMITS });
    let kit = h.open(5, authority(ALL));
    assert_eq!(h.call(kit, read(b"/etc/passwd")), Outcome::Outside, "a lost fact changes no answer");
    assert_eq!(h.step(Event::Close { kit }), Some(Request::Closed { session: Token::new(5) }));
    assert_eq!(h.domain.pop_fact(), Some(Fact::Opened { session: Token::new(5) }));
    assert_eq!(h.domain.pop_fact(), None);
    assert_eq!(h.domain.facts_lost(), 2);
}

#[test]
fn a_command_of_a_kit_without_modify_sees_every_repository_read_only() {
    let mut h = Harness::new(LIMITS);
    let kit = h.open(1, authority(Grants { inspect: true, modify: false, shell: true }));
    let (_, op, _) = spawned(&mut h, kit, None);
    match op {
        Op::Spawn { roots, .. } => {
            assert_eq!(roots.len(), 3);
            for root in &roots {
                assert!(!root.writable, "{root:?}");
            }
        }
        op @ (Op::Load { .. } | Op::Scan { .. } | Op::Store { .. } | Op::Search { .. }) => {
            panic!("expected a spawn, not {op:?}")
        }
    }
}
