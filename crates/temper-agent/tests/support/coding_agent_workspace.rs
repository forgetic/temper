//! Shared git/checkout helpers for the coding-agent jig tests.
//!
//! These build throwaway on-disk git checkouts so the native agent loop can run
//! against a real working tree. They are test-only scaffolding, not part of the
//! crate's API; each `jig_coding_agent` scenario pulls them in via `#[path]`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The single repo's sibling dir under the workspace root (cwd). A single-repo
/// job is a one-entry manifest; the repo still lives in its own subdir, as in
/// the coordinated multi-repo layout (ADR 0023).
pub const REPO_DIR: &str = "demo";

/// A temporary workspace root (the agent's cwd) that cleans itself up on drop.
pub struct TempCheckout {
    path: PathBuf,
}

impl TempCheckout {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "anvil-{name}-{}-{}",
            std::process::id(),
            unique_nanos()
        ));
        fs::create_dir_all(&path).expect("create temp checkout");
        Self { path }
    }

    /// The workspace root — the agent's cwd.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The repo checkout, a subdir of the workspace root.
    pub fn repo_path(&self) -> PathBuf {
        self.path.join(REPO_DIR)
    }

    pub fn init_git(&self) {
        fs::create_dir_all(self.repo_path()).expect("create repo dir");
        fs::write(self.repo_path().join("README.md"), "# demo\n").expect("seed README");
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.email", "jig@example.invalid"]);
        self.git(&["config", "user.name", "Jig Test"]);
        self.git(&["add", "README.md"]);
        self.git(&["commit", "-m", "seed"]);
    }

    pub fn git(&self, args: &[&str]) -> String {
        run_git(&self.repo_path(), args)
    }
}

impl Drop for TempCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn unique_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}

/// Seeds a git repo with one commit at `<root>/<dir>`.
pub fn seed_repo(root: &Path, dir: &str) {
    let repo = root.join(dir);
    fs::create_dir_all(&repo).expect("create repo dir");
    fs::write(repo.join("README.md"), format!("# {dir}\n")).expect("seed README");
    run_git(&repo, &["init", "-b", "main"]);
    run_git(&repo, &["config", "user.email", "jig@example.invalid"]);
    run_git(&repo, &["config", "user.name", "Jig Test"]);
    run_git(&repo, &["add", "README.md"]);
    run_git(&repo, &["commit", "-m", "seed"]);
}

/// Runs `git <args>` in `dir`, asserting success and returning stdout.
pub fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout is utf8")
}
