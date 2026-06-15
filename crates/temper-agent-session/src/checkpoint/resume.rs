// SPDX-License-Identifier: MPL-2.0

//! Recovering a prior run's pushed checkpoints from the prepared branch.
//!
//! A re-dispatched agent finds checkpoint commits on the prepared branch,
//! resumes its step numbering from them, and tells the model what already
//! landed.

use super::Checkpointer;

/// A prior run's pushed checkpoints, recovered from the prepared branch.
pub(crate) struct ResumeState {
    /// Commits beyond the base branch.
    pub(crate) commits: u32,
    /// The highest step index encoded in checkpoint commit subjects (or the
    /// commit count + 1 when none parse), so new markers stay monotonic.
    pub(crate) last_step: u32,
    /// Current branch head (what the checkpoints pushed).
    pub(crate) head_sha: String,
    /// One subject line per checkpoint commit, oldest first.
    pub(crate) log: String,
}

impl Checkpointer {
    /// Inspects each writable repo's prepared branch for checkpoints a previous
    /// run pushed, aggregating across repos. Resume is keyed off the whole
    /// workspace: any repo with checkpoint commits means the run is resuming.
    pub(crate) fn detect_resume(&self) -> Option<ResumeState> {
        let mut total_commits = 0u32;
        let mut last_step = 0u32;
        let mut head_sha = None;
        let mut logs = Vec::new();
        for repo in &self.repos {
            let range = format!("origin/{}..HEAD", repo.base_branch);
            let Some(count) = self
                .git_in(&repo.dir, &["rev-list", "--count", &range])
                .ok()
                .and_then(|out| out.trim().parse::<u32>().ok())
            else {
                continue;
            };
            if count == 0 {
                continue;
            }
            let log = self
                .git_in(&repo.dir, &["log", "--reverse", "--format=%s", &range])
                .ok()
                .map(|out| out.trim().to_string())
                .unwrap_or_default();
            let repo_last_step = log
                .lines()
                .filter_map(parse_checkpoint_step)
                .max()
                .unwrap_or(count + 1);
            total_commits += count;
            last_step = last_step.max(repo_last_step);
            if head_sha.is_none() {
                head_sha = self
                    .git_in(&repo.dir, &["rev-parse", "HEAD"])
                    .ok()
                    .map(|out| out.trim().to_string());
            }
            if !log.is_empty() {
                logs.push(format!("{}:\n{log}", repo.dir));
            }
        }
        if total_commits == 0 {
            return None;
        }
        Some(ResumeState {
            commits: total_commits,
            last_step,
            head_sha: head_sha.unwrap_or_default(),
            log: logs.join("\n"),
        })
    }
}

/// Parses `checkpoint(step N): …` commit subjects.
fn parse_checkpoint_step(subject: &str) -> Option<u32> {
    subject
        .strip_prefix("checkpoint(step ")?
        .split_once(')')?
        .0
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::parse_checkpoint_step;

    #[test]
    fn parses_checkpoint_subjects() {
        assert_eq!(
            parse_checkpoint_step("checkpoint(step 3): work in progress (turn 2)"),
            Some(3)
        );
        assert_eq!(parse_checkpoint_step("checkpoint(step 12): x"), Some(12));
        assert_eq!(parse_checkpoint_step("Implement pr-for-code-7"), None);
        assert_eq!(parse_checkpoint_step("checkpoint(step ): x"), None);
    }
}
