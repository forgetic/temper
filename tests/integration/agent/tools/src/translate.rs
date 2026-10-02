//! The mapping between the tools' vocabulary and the fake checkout's: what the
//! protocol layer (splitting the paths the LLM wrote) and io (running file
//! operations, naming roots and versions) do, without the bytes and the
//! kernel.

use temper_agent_model_tools::{
    Done, Entry, Exit, Expect, Fault, Hit, Kind, Name, Op, Part, Path, Place, Root, Var, Version,
};
use temper_checkout_fake as fake;
use temper_lib::Token;

/// The path the LLM wrote, split at its slashes, as the protocol layer does.
#[must_use]
pub fn path(text: &[u8]) -> Path {
    let mut parts = Vec::new();
    for part in text.split(|byte| *byte == b'/') {
        match part {
            b"" => {}
            b"." => parts.push(Part::Current),
            b".." => parts.push(Part::Parent),
            name => parts.push(Part::Name { name: name_of(name) }),
        }
    }
    Path { absolute: text.first() == Some(&b'/'), parts: parts.into() }
}

/// The names of the absolute path `text`.
#[must_use]
pub fn names(text: &[u8]) -> Box<[Name]> {
    let mut names = Vec::new();
    for part in path(text).parts {
        match part {
            Part::Name { name } => names.push(name),
            Part::Current | Part::Parent => panic!("{:?} is not normalised", String::from_utf8_lossy(text)),
        }
    }
    names.into()
}

fn name_of(bytes: &[u8]) -> Name {
    Name::new(bytes.into()).expect("the protocol layer refuses what is not a name")
}

/// The token io gives the fake's root `root`, and back.
#[must_use]
pub fn token(root: u64) -> Token {
    Token::new(root)
}

fn root(token: Token) -> u64 {
    token.raw()
}

/// io's version for the fake's, and back.
#[must_use]
pub fn version(version: u64) -> Version {
    Version::new([version, 0, 0, 0])
}

fn fake_version(version: Version) -> u64 {
    let [version, ..] = version.raw();
    version
}

/// A command io starts for a spawn, with how much of its output to keep:
/// what runs, its first bytes and its last.
pub struct Started {
    pub process: fake::Process,
    pub head: u32,
    pub tail: u32,
}

/// Starts the command of a spawn on the checkout, as io would, or the
/// terminal that says why it did not start.
pub fn spawn(
    checkout: &fake::Checkout,
    cwd: &Place,
    command: &[u8],
    env: &[Var],
    roots: &[Root],
    (head, tail): (u32, u32),
) -> Result<Started, Done> {
    let env: Vec<(Vec<u8>, Vec<u8>)> = env.iter().map(|var| (var.name.to_vec(), var.value.to_vec())).collect();
    let roots: Vec<(u64, bool)> = roots.iter().map(|seen| (seen.root.raw(), seen.writable)).collect();
    match checkout.spawn(root(cwd.root), &cwd.path, command, &env, &roots) {
        Ok(process) => Ok(Started { process, head, tail }),
        Err(failure) => Err(done(failure)),
    }
}

/// How io ends a command that ran: with `exit`, or timed out if `None`, having
/// written `output`, of which it keeps the head and the tail.
#[must_use]
pub fn exited(exit: Option<fake::Exit>, output: &[u8], head: u32, tail: u32) -> Done {
    let exit = match exit {
        Some(fake::Exit::Code(code)) => Exit::Code { code },
        Some(fake::Exit::Signal(signal)) => Exit::Signal { signal },
        None => Exit::TimedOut,
    };
    let head = usize::try_from(head).expect("a small head").min(output.len());
    let tail = usize::try_from(tail).expect("a small tail").min(output.len() - head);
    let dropped = u64::try_from(output.len() - head - tail).expect("a small output");
    let (kept, rest) = output.split_at(head);
    Done::Exited { exit, head: kept.into(), tail: rest[rest.len() - tail..].into(), dropped }
}

/// Runs `op` on the checkout, as io would, and returns its terminal. A spawn
/// is started instead ([`spawn`]).
pub fn perform(checkout: &mut fake::Checkout, op: Op) -> Done {
    match op {
        Op::Load { at, max } => match checkout.load(root(at.root), &at.path, u64::from(max)) {
            Ok((content, found)) => Done::Loaded { content: content.into(), version: version(found) },
            Err(failure) => done(failure),
        },
        Op::Scan { at, max } => {
            let max = usize::try_from(max).expect("a small listing");
            match checkout.scan(root(at.root), &at.path, max) {
                Ok((entries, more)) => Done::Scanned { entries: entries.into_iter().map(entry).collect(), more },
                Err(failure) => done(failure),
            }
        }
        Op::Store { at, content, expect } => {
            let expect = match expect {
                Expect::Absent => fake::Expect::Absent,
                Expect::Is { version } => fake::Expect::Is(fake_version(version)),
            };
            match checkout.store(root(at.root), &at.path, &content, expect) {
                Ok(stored) => Done::Stored { version: version(stored) },
                Err(failure) => done(failure),
            }
        }
        Op::Search { at, pattern, glob, hits, bytes } => {
            let limits = (usize::try_from(hits).expect("small"), usize::try_from(bytes).expect("small"));
            match checkout.search(root(at.root), &at.path, &pattern, glob.as_deref(), limits) {
                Ok(found) => Done::Found { hits: found.hits.into_iter().map(hit).collect(), more: found.more },
                // rg exits with 2 for a pattern it cannot read.
                Err(fake::Searched::Unreadable(stderr)) => {
                    let head = usize::try_from(bytes).expect("small").min(stderr.len());
                    let dropped = u64::try_from(stderr.len() - head).expect("small");
                    let head = stderr[..head].into();
                    Done::Exited { exit: Exit::Code { code: 2 }, head, tail: Box::new([]), dropped }
                }
                Err(fake::Searched::Failed(failure)) => done(failure),
            }
        }
        Op::Spawn { .. } => panic!("a spawn is started, not run"),
    }
}

fn hit((path, line, text): (Vec<u8>, usize, Vec<u8>)) -> Hit {
    Hit { path: path.into(), line: u32::try_from(line).expect("a short file"), text: text.into() }
}

fn entry((name, kind): (Vec<u8>, fake::Kind)) -> Entry {
    let kind = match kind {
        fake::Kind::File => Kind::File,
        fake::Kind::Directory => Kind::Directory,
        fake::Kind::Link => Kind::Link,
        fake::Kind::Special => Kind::Other,
    };
    Entry { name: name_of(&name), kind }
}

fn done(failure: fake::Failure) -> Done {
    match failure {
        fake::Failure::Missing => Done::Missing,
        fake::Failure::NotFile => Done::NotFile,
        fake::Failure::NotDirectory => Done::NotDirectory,
        fake::Failure::TooLarge { size } => Done::TooLarge { size },
        fake::Failure::Escapes => Done::Escapes,
        fake::Failure::Linked => Done::Linked,
        fake::Failure::Loop => Done::Failed { fault: Fault::Other },
        fake::Failure::Conflict { now } => Done::Conflict { now: now.map(version) },
    }
}
