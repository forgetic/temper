//! What the memory tests build (programming-model.md, 6.3): small limits, and
//! the largest of each thing they take, an authority, a path and a file, so
//! that a domain holds as much as its worst case allows.

use skein_lib::{Duration, Token};
use temper_agent_domain_tools::{Authority, Call, Grants, Limits, Name, Part, Path, Repo, Var};

pub const LIMITS: Limits = Limits {
    kits: 2,
    calls: 3,
    repos: 2,
    path_bytes: 63,
    known_files: 4,
    file_bytes: 256,
    read_bytes: 64,
    list_entries: 4,
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

pub const GRANTS: Grants = Grants { inspect: true, modify: true, shell: true };

#[must_use]
pub fn name(bytes: &[u8]) -> Name {
    Name::new(bytes.into()).expect("a test name")
}

/// The longest authority `limits` take: a working directory of as many names
/// as fit, a repository at the root, so that a place's path is as long as an
/// absolute one, and the others at mounts as long as they may be.
#[must_use]
pub fn authority(limits: &Limits) -> Authority {
    let len = usize::try_from(limits.path_bytes).expect("a small limit");
    let names = len.div_ceil(2);
    let cwd: Box<[Name]> = (0..names).map(|_| name(b"a")).collect();
    let mut repos = vec![Repo { mount: Box::new([]), root: Token::new(0), writable: true }];
    for repo in 1..limits.repos {
        let mount = format!("{repo:0>len$}");
        repos.push(Repo { mount: Box::new([name(mount.as_bytes())]), root: Token::new(repo.into()), writable: false });
    }
    // An environment as large as it may be, in variables of a byte each.
    let vars = limits.env_bytes / 4;
    let env =
        (0..vars).map(|var| Var { name: format!("{var:x}").into_bytes().into(), value: b"v"[..].into() }).collect();
    Authority { cwd, repos: repos.into(), grants: GRANTS, env }
}

/// The path of a file whose absolute path is exactly `path_bytes` long,
/// distinct for each `file`.
#[must_use]
pub fn path(limits: &Limits, file: u64) -> Path {
    let len = usize::try_from(limits.path_bytes - 1).expect("a small limit");
    let name = format!("f{file:0>len$}");
    let parts = Box::new([Part::Name { name: Name::new(name.into_bytes().into()).expect("a name") }]);
    Path { absolute: true, parts }
}

#[must_use]
pub fn read(limits: &Limits, file: u64) -> Call {
    Call::Read { path: path(limits, file), skip: 0, lines: None }
}

/// The most a file holds.
#[must_use]
pub fn full(limits: &Limits, byte: u8) -> Box<[u8]> {
    vec![byte; usize::try_from(limits.file_bytes).expect("a small limit")].into()
}

/// A write of a file the kit read, so that it has a version to expect.
#[must_use]
pub fn write(limits: &Limits, file: u64) -> Call {
    Call::Write { path: path(limits, file), content: full(limits, b'x') }
}

/// An edit of a file the kit read, with snippets as long as they may be,
/// which the job holds while it loads the file.
#[must_use]
pub fn edit(limits: &Limits, file: u64) -> Call {
    Call::Edit { path: path(limits, file), old: full(limits, b'x'), new: full(limits, b'y'), all: true }
}
