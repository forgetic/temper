// SPDX-License-Identifier: MPL-2.0

//! The blocking commit + push that a checkpoint performs off the loop thread.
//!
//! [`CheckpointJob`] is the owned snapshot handed to `spawn_blocking`;
//! `checkpoint_sync` and `git_in` run the git plumbing in each writable repo.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::time::Instant;

use temper_protocol_agent::PullRequestFreshness;

use crate::config::DEFAULT_CHECKPOINT_INTERVAL;

use super::{CheckpointRepo, Checkpointer};

/// The owned snapshot a blocking checkpoint runs from.
pub(super) struct CheckpointJob {
    pub(super) cwd: PathBuf,
    pub(super) repos: Vec<CheckpointRepo>,
    pub(super) correlation_key: String,
    pub(super) freshness_url: Option<String>,
    pub(super) pull_request_freshness: Option<PullRequestFreshness>,
}

impl CheckpointJob {
    pub(super) fn into_checkpointer(self) -> Checkpointer {
        // This snapshot only runs `checkpoint_sync` off-thread; the
        // backstop/timing fields are unused here.
        Checkpointer {
            cwd: self.cwd,
            repos: self.repos,
            correlation_key: self.correlation_key,
            freshness_url: self.freshness_url,
            pull_request_freshness: self.pull_request_freshness,
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
            // The git author identity and push credential (`http.extraheader`)
            // are configured by the worker in this repo's local `.git/config`
            // before the agent was spawned, so the commit/push use them without
            // the agent ever holding the push token.
            self.git_in(
                &repo.dir,
                &[
                    "commit",
                    "-m",
                    &format!("checkpoint(step {step}): {summary}"),
                ],
            )?;
            let push_ref = format!("HEAD:refs/heads/{}", repo.branch);
            self.git_in(&repo.dir, &["push", "origin", &push_ref])?;
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
    /// stdout. The push credential lives in the repo's local `.git/config` (set
    /// by the worker), never on argv, so neither the command nor git's stderr
    /// carries the push token.
    pub(super) fn git_in(&self, dir: &str, args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(self.cwd.join(dir))
            .output()
            .map_err(|error| format!("spawn git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git {} failed ({}): {}",
                args.first().copied().unwrap_or(""),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8(output.stdout).map_err(|error| format!("git output not UTF-8: {error}"))
    }
}
