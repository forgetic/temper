// SPDX-License-Identifier: MPL-2.0

//! The blocking commit + push that a checkpoint performs off the loop thread.
//!
//! [`CheckpointJob`] is the owned snapshot handed to `spawn_blocking`;
//! `checkpoint_sync` and `git_in` run the git plumbing in each writable repo.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::time::Instant;

use super::{CheckpointRepo, Checkpointer, DEFAULT_CHECKPOINT_INTERVAL};

/// The owned snapshot a blocking checkpoint runs from.
pub(super) struct CheckpointJob {
    pub(super) cwd: PathBuf,
    pub(super) repos: Vec<CheckpointRepo>,
    pub(super) correlation_key: String,
    pub(super) user: Option<String>,
    pub(super) email: Option<String>,
    pub(super) token: Option<String>,
}

impl CheckpointJob {
    pub(super) fn into_checkpointer(self) -> Checkpointer {
        // This snapshot only runs `checkpoint_sync` off-thread; the
        // backstop/timing fields are unused here.
        Checkpointer {
            cwd: self.cwd,
            repos: self.repos,
            correlation_key: self.correlation_key,
            user: self.user,
            email: self.email,
            token: self.token,
            deadline: None,
            interval: DEFAULT_CHECKPOINT_INTERVAL,
            last_checkpoint: Mutex::new(Instant::now()),
            step: AtomicU32::new(0),
        }
    }
}

impl Checkpointer {
    /// One commit+push checkpoint across every writable repo; returns the first
    /// pushed sha (the primary's), or `None` when no repo had staged changes.
    /// `label` is the model's milestone summary (a backstop checkpoint passes
    /// `None`).
    pub(super) fn checkpoint_sync(
        &self,
        step: u32,
        label: Option<&str>,
    ) -> Result<Option<String>, String> {
        let summary = label.unwrap_or("work in progress");
        let mut pushed = None;
        for repo in &self.repos {
            self.git_in(&repo.dir, &["add", "-A"])?;
            // Exit 0 = nothing staged in this repo.
            if self
                .git_in(&repo.dir, &["diff", "--cached", "--quiet"])
                .is_ok()
            {
                continue;
            }
            let user = self.user.as_deref().unwrap_or("temper-agent");
            let email = self.email.as_deref().unwrap_or("temper-agent@localhost");
            self.git_in(
                &repo.dir,
                &[
                    "-c",
                    &format!("user.name={user}"),
                    "-c",
                    &format!("user.email={email}"),
                    "commit",
                    "-m",
                    &format!("checkpoint(step {step}): {summary}"),
                ],
            )?;
            let push_ref = format!("HEAD:refs/heads/{}", repo.branch);
            match self.token.as_deref() {
                Some(token) => self.git_in(
                    &repo.dir,
                    &[
                        "-c",
                        &format!("http.extraheader=AUTHORIZATION: token {token}"),
                        "push",
                        "origin",
                        &push_ref,
                    ],
                )?,
                None => self.git_in(&repo.dir, &["push", "origin", &push_ref])?,
            };
            let sha = self
                .git_in(&repo.dir, &["rev-parse", "HEAD"])?
                .trim()
                .to_string();
            if pushed.is_none() {
                pushed = Some(sha);
            }
        }
        Ok(pushed)
    }

    /// Runs git in a repo's checkout dir (under the workspace root), returning
    /// stdout. Errors carry the command and stderr with the push token redacted.
    pub(super) fn git_in(&self, dir: &str, args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(self.cwd.join(dir))
            .output()
            .map_err(|error| format!("spawn git: {error}"))?;
        if !output.status.success() {
            let mut detail = format!(
                "git {} failed ({}): {}",
                args.first().copied().unwrap_or(""),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            if let Some(token) = self.token.as_deref() {
                detail = detail.replace(token, "<redacted>");
            }
            return Err(detail);
        }
        String::from_utf8(output.stdout).map_err(|error| format!("git output not UTF-8: {error}"))
    }
}
