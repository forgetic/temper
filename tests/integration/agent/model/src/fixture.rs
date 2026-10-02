//! The forge the engine's workspaces are drawn from, and the commands and
//! checks that run in the working trees the worker checks out of it.
//!
//! Each job has a repository of its own on the forge, named for it: the code
//! (an answer of 42, which its checks want to be 43), a guide for LLMs that
//! cues the job's script, and the checks: an executable at `.temper/pre-pr`,
//! which is a list of files and what each must hold. Executables start with
//! `#!`. Beside them, `docs` holds documentation only. Every repository's
//! default branch is the engine's base branch.

use std::time::Duration;

use temper_checkout_fake::git::{Forge, Tree};
use temper_checkout_fake::{self as fake, Checkout, Program};
use temper_fake_engine_model::{BASE, Origin};

use crate::script::{self, Job};

/// Where the checks are, beneath a repository's root.
pub const CHECKS: &[u8] = b".temper/pre-pr";

/// The code, as each run finds it, and what its checks want of it.
pub const CODE: (&[u8], &[u8]) = (b"src/lib.rs", b"pub fn answer() -> u32 { 42 }\n");
pub const WANTED: &[u8] = b"43";

/// The repository of documentation only.
pub const DOCS: &[u8] = b"docs";

/// The commands the scripts run, how long each takes, what it writes and how
/// it ends.
const COMMANDS: [(&[u8], u64, &[u8], u8); 3] = [
    (b"cargo test", 200, b"test result: ok. 1 passed\n", 0),
    (b"ls", 10, b"AGENTS.md README.md src\n", 0),
    (b"sleep", 3_600_000, b"", 0),
];

/// Scripts the commands, once for the whole disk.
pub fn script(disk: &mut Checkout) {
    for (command, millis, output, code) in COMMANDS {
        let program = Program {
            duration: Duration::from_millis(millis),
            output: output.to_vec(),
            exit: fake::Exit::Code(code),
            changes: Vec::new(),
        };
        disk.program(command, program);
    }
}

/// The name of `job`'s repository, which is the directory a workspace gives
/// it.
#[must_use]
pub const fn name(job: Job) -> &'static [u8] {
    match job {
        Job::Coding => b"coding",
        Job::Review => b"review",
        Job::Delegating => b"delegating",
        Job::Spending => b"spending",
        Job::Wandering => b"wandering",
    }
}

/// The job whose repository is named `name`, if it is one's.
#[must_use]
pub fn job(name: &[u8]) -> Option<Job> {
    script::JOBS.into_iter().find(|job| self::name(*job) == name)
}

/// The engine's origin for the repository named `name`.
#[must_use]
pub fn origin(name: &[u8]) -> Origin {
    Origin { name: name.into(), remote: [b"ai/", name].concat().into() }
}

/// Seeds `forge` with every repository of `origins` once, each at a first
/// commit on the base branch; and returns the first one's.
pub fn seed(forge: &mut Forge, origins: &[Origin]) -> u64 {
    let mut first = None;
    for (place, origin) in origins.iter().enumerate() {
        if origins[..place].iter().any(|before| before.remote == origin.remote) {
            continue;
        }
        let tree = match job(&origin.name) {
            Some(job) => code(script::cue(job)),
            None => docs(),
        };
        let commit = forge.repository(&origin.remote, BASE, tree);
        first.get_or_insert(commit);
    }
    first.expect("the engine draws from a repository")
}

/// A repository of code, its guide cued with `cue`.
fn code(cue: Option<&[u8]>) -> Tree {
    let guide = [cue.unwrap_or_default(), b" The answer is in src/lib.rs; the checks want it to be 43.\n"].concat();
    let checks = [b"#!checks\n", CODE.0, b" ", WANTED, b"\n"].concat();
    Tree::from([
        (b"AGENTS.md".to_vec(), guide),
        (b"README.md".to_vec(), b"temper: the answer\n".to_vec()),
        (CODE.0.to_vec(), CODE.1.to_vec()),
        (CHECKS.to_vec(), checks),
    ])
}

fn docs() -> Tree {
    Tree::from([
        (b"AGENTS.md".to_vec(), b"Documentation only.\n".to_vec()),
        (b"guide.md".to_vec(), b"docs: the guide\n".to_vec()),
    ])
}

/// Whether the file `content` is executable.
#[must_use]
pub fn executable(content: &[u8]) -> bool {
    content.starts_with(b"#!")
}

/// What the checks `program` find in the repository at `root`: whether they
/// pass, and what they write. Each line after the first names a file and what
/// it must hold.
#[must_use]
pub fn check(disk: &Checkout, root: u64, program: &[u8]) -> (bool, Vec<u8>) {
    let mut passed = true;
    let mut output = Vec::new();
    for line in program.split(|byte| *byte == b'\n').skip(1).filter(|line| !line.is_empty()) {
        let Some(space) = line.iter().position(|byte| *byte == b' ') else {
            continue;
        };
        let (path, wanted) = (&line[..space], &line[space + 1..]);
        let held = match disk.load(root, path, u64::MAX) {
            Ok((content, _)) => content.windows(wanted.len()).any(|window| window == wanted),
            Err(_) => false,
        };
        if held {
            output.extend_from_slice(&[b"ok: ", path, b"\n"].concat());
        } else {
            passed = false;
            output.extend_from_slice(&[b"FAILED: ", path, b" does not hold ", wanted, b"\n"].concat());
        }
    }
    (passed, output)
}
