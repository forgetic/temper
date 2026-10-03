//! What the deployment's repositories hold, and the commands and checks
//! that run in the working trees the worker checks out of them.
//!
//! Every repository holds the code (an answer of 42, which its checks want
//! to be 43), a guide for LLMs, the checks (an executable at
//! `.temper/pre-pr`, which is a list of files and what each must hold;
//! executables start with `#!`), and the file the forge's CI reads, which
//! says green: a change that leaves it so passes CI. A run's script is cued
//! by the guidance of its step, not by its repository ([`crate::desk`]).

use std::time::Duration;

use temper_checkout_fake::git::Tree;
use temper_checkout_fake::{self as fake, Checkout, Program};
use temper_engine_domain_tests::deployment::{CUE, GREEN};

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

/// What every repository of the deployment holds on its default branch: the
/// code, a guide, the checks, and the file CI reads, green.
#[must_use]
pub fn tree() -> Tree {
    let guide = b"The answer is in src/lib.rs; the checks want it to be 43.\n".to_vec();
    let checks = [b"#!checks\n", CODE.0, b" ", WANTED, b"\n"].concat();
    Tree::from([
        (b"AGENTS.md".to_vec(), guide),
        (b"README.md".to_vec(), b"temper: the answer\n".to_vec()),
        (CODE.0.to_vec(), CODE.1.to_vec()),
        (CHECKS.to_vec(), checks),
        (CUE.to_vec(), GREEN.to_vec()),
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
