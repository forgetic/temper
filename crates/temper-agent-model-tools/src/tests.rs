//! Feed the model events, inspect the requests that come out; and the pure
//! functions on paths, by tables.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::authority::{self, Located};
use crate::path;
use crate::{
    Authority, Call, Effect, Event, Grants, Limits, Model, Name, Outcome, Part, Path, Place, Refusal, Repo, Request,
    effect, max_out, step, worst_case,
};

const LIMITS: Limits = Limits {
    kits: 2,
    calls: 4,
    repos: 4,
    path_bytes: 64,
    known_files: 8,
    file_bytes: 1024,
    read_bytes: 256,
    list_entries: 16,
    file_timeout: Duration::from_secs(10),
};

const ALL: Grants = Grants { inspect: true, modify: true, shell: true };

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness {
            model: Model::new(&limits),
            env: Env { now: Time::ZERO, limits },
            out: Queue::with_capacity(max_out(&limits)),
        }
    }

    fn step(&mut self, event: Event) -> Option<Request> {
        step(&mut self.model, &self.env, event, &mut self.out);
        let request = self.out.pop();
        assert!(self.out.is_empty(), "one request at most");
        request
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
    }
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
    assert_eq!(h.model.kits(), 0);
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
    assert_eq!(h.model.kits(), 1);
    assert_eq!(h.step(Event::Close { kit }), Some(Request::Closed { session: Token::new(5) }));
    assert_eq!(h.model.kits(), 1, "a closed kit stays until the reclaim point");
    h.model.reclaim();
    assert_eq!(h.model.kits(), 0);
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
        (inspect, read(b"a/very/long/path/that/does/not/fit/in/the/sixty/four/bytes/allowed"), Outcome::TooLong),
        // What passes the entrance does not run yet.
        (inspect, read(b"src/lib.rs"), Outcome::Unsupported),
        (modify, write(b"src/big.rs", 1024), Outcome::Unsupported),
        (ALL, shell, Outcome::Unsupported),
    ];
    let mut h = Harness::new(Limits { kits: 16, ..LIMITS });
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
