//! The mapping between the tools' vocabulary and the fake checkout's: what the
//! protocol layer (splitting the paths the LLM wrote) and io (running file
//! operations, naming roots and versions) do, without the bytes and the
//! kernel.

use temper_agent_model_tools::{Done, Entry, Expect, Fault, Kind, Name, Op, Part, Path, Version};
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

/// Runs `op` on the checkout, as io would, and returns its terminal.
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
    }
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
        fake::Failure::Loop => Done::Failed { fault: Fault::Other },
        fake::Failure::Conflict { now } => Done::Conflict { now: now.map(version) },
    }
}
