//! Forgejo CI marker helpers used by provisioning.
//!
//! Production role workers no longer carry a synthetic PR-prep adapter. Product
//! code branches must come from a declared-and-bound coding workspace, and PR
//! review/merge workers keep the diff guard enabled by default. This module only
//! retains the commit-message marker helpers used to make the provisioned
//! default branch's CI workflow pass after setup.

use temper_forge::CommitFile;

use crate::provision::CI_WORKFLOW;

const WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";

const CI_SENTINEL_DIR: &str = ".temper-ci";
pub const CI_PASS_MARKER: &str = "[ci-pass]";

/// Builds the Forgejo seed commits applied to a freshly created repository's
/// default branch: the marker CI workflow (`runs-on: host`, the `[ci-pass]`
/// commit-message gate) followed by a sentinel commit whose message carries the
/// `[ci-pass]` marker so the just-installed workflow passes on the default
/// branch.
///
/// Both mutate repository history, so a caller provisioning onto a repo that
/// owns its own CI (`--existing-repo`) must NOT apply them; the
/// orchestration in `temper-provision` already skips a plan's `seed_commits`
/// when `existing_repo` is set.
pub fn ci_seed_commits(branch: &str) -> Vec<CommitFile> {
    vec![
        CommitFile {
            path: WORKFLOW_PATH.to_string(),
            contents: CI_WORKFLOW.as_bytes().to_vec(),
            message: "add CI workflow (runs-on: host)".to_string(),
            branch: branch.to_string(),
        },
        ci_sentinel_commit(branch),
    ]
}

/// Builds the CI sentinel commit for `branch`: a small file under
/// `.temper-ci/<branch>.txt` whose commit message carries the `[ci-pass]`
/// marker, so the marker-gated CI workflow passes on the default branch right
/// after provisioning.
pub fn ci_sentinel_commit(branch: &str) -> CommitFile {
    let safe: String = branch
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    CommitFile {
        path: format!("{CI_SENTINEL_DIR}/{safe}.txt"),
        contents: format!("ci pass marker for {branch}\n").into_bytes(),
        message: format!("ci pass for {branch} {CI_PASS_MARKER}"),
        branch: branch.to_string(),
    }
}
