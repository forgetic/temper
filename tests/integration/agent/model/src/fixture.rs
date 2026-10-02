//! The checkout the worker prepares for each run, in the fake checkout, and
//! the commands and checks that run in it.
//!
//! Each run's repositories are directories of their own, `j<job>/<name>`, and
//! roots io names. The first, the one the work is about, holds the code (an
//! answer of 42, which its checks want to be 43), a guide for LLMs that cues
//! the job's script, and the checks: an executable at `.temper/pre-pr`, which
//! is a list of files and what each must hold. Executables start with `#!`.
//! The others hold documentation.

use std::time::Duration;

use temper_checkout_fake::{self as fake, Checkout, Program};

/// Where the checks are, beneath a repository's root.
pub const CHECKS: &[u8] = b".temper/pre-pr";

/// The code, as each run finds it, and what its checks want of it.
pub const CODE: (&[u8], &[u8]) = (b"src/lib.rs", b"pub fn answer() -> u32 { 42 }\n");
pub const WANTED: &[u8] = b"43";

/// The commands the scripts run, how long each takes, what it writes and how
/// it ends.
const COMMANDS: [(&[u8], u64, &[u8], u8); 3] = [
    (b"cargo test", 200, b"test result: ok. 1 passed\n", 0),
    (b"ls", 10, b"AGENTS.md README.md src\n", 0),
    (b"sleep", 3_600_000, b"", 0),
];

/// Scripts the commands, once for the whole checkout.
pub fn script(checkout: &mut Checkout) {
    for (command, millis, output, code) in COMMANDS {
        let program = Program {
            duration: Duration::from_millis(millis),
            output: output.to_vec(),
            exit: fake::Exit::Code(code),
            changes: Vec::new(),
        };
        checkout.program(command, program);
    }
}

/// Seeds the repository `name` of the run of `job`, the first of its checkout
/// if `first`, its guide cued with `cue`; and makes it a root, returning io's
/// name for it.
pub fn seed(checkout: &mut Checkout, job: u64, name: &[u8], first: bool, cue: Option<&[u8]>) -> u64 {
    let at = [format!("j{job}/").as_bytes(), name].concat();
    let file = |path: &[u8]| [&at[..], b"/", path].concat();
    checkout.mkdir(&at);
    if first {
        let guide = [cue.unwrap_or_default(), b" The answer is in src/lib.rs; the checks want it to be 43.\n"].concat();
        checkout.write(&file(b"AGENTS.md"), &guide);
        checkout.write(&file(b"README.md"), b"temper: the answer\n");
        checkout.write(&file(CODE.0), CODE.1);
        let checks = [b"#!checks\n", CODE.0, b" ", WANTED, b"\n"].concat();
        checkout.write(&file(CHECKS), &checks);
    } else {
        checkout.write(&file(b"AGENTS.md"), b"Documentation only.\n");
        checkout.write(&file(b"guide.md"), b"docs: the guide\n");
    }
    checkout.root(&at)
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
pub fn check(checkout: &Checkout, root: u64, program: &[u8]) -> (bool, Vec<u8>) {
    let mut passed = true;
    let mut output = Vec::new();
    for line in program.split(|byte| *byte == b'\n').skip(1).filter(|line| !line.is_empty()) {
        let Some(space) = line.iter().position(|byte| *byte == b' ') else {
            continue;
        };
        let (path, wanted) = (&line[..space], &line[space + 1..]);
        let held = match checkout.load(root, path, u64::MAX) {
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
