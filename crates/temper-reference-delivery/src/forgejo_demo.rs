//! Forgejo-specific reference-delivery demo bits: the marker CI workflow, its
//! seed commits, and the demo intake-issue defaults.
//!
//! These make the *demo* repo's CI go green right after provisioning and supply
//! the demo "add a greeting" intake story. A real repo owns its own CI (and
//! `--existing-repo` provisioning skips these commits), so they belong with the
//! reference-delivery demo rather than in any provisioning crate.
//!
//! Production role workers no longer carry a synthetic PR-prep adapter; product
//! code branches must come from a declared-and-bound coding workspace. These
//! helpers only retain the commit-message marker that makes the provisioned
//! default branch's CI workflow pass after setup.

use temper_forge::CommitFile;

/// Demo intake issue title (the "add a greeting" reference-delivery story).
pub const DEFAULT_INTAKE_TITLE: &str = "Add a configurable greeting to the service banner";
/// Demo intake issue body (the "add a greeting" reference-delivery story).
pub const DEFAULT_INTAKE_BODY: &str = "As an operator I want the service banner to show a \
configurable greeting so I can tell environments apart at a glance.\n\n\
Acceptance: a `BANNER_GREETING` setting whose value is printed on startup, \
defaulting to the current text when unset.";

const WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";
const CI_SENTINEL_DIR: &str = ".temper-ci";

/// Commit-message marker the demo CI workflow gates on.
pub const CI_PASS_MARKER: &str = "[ci-pass]";

// NOTE: this MUST be a raw string. A normal string literal with `\<newline>`
// continuations strips the leading whitespace of each continued source line,
// which is exactly the YAML indentation — the committed workflow then lands
// flush-left, is invalid, and Forgejo silently detects no workflow (no CI runs
// ever fire). See agent lesson 0013.
pub const CI_WORKFLOW: &str = r#"name: ci
on: [push]
jobs:
  build:
    runs-on: host
    steps:
      - name: gate on commit message marker
        run: |
          python3 - <<'PY'
          import json
          import os
          import sys
          import urllib.request

          api = os.environ["GITHUB_API_URL"]
          repo = os.environ["GITHUB_REPOSITORY"]
          sha = os.environ["GITHUB_SHA"]
          token = os.environ["GITHUB_TOKEN"]

          req = urllib.request.Request(
              f"{api}/repos/{repo}/git/commits/{sha}",
              headers={
                  "Authorization": f"token {token}",
                  "Accept": "application/json",
              },
          )
          with urllib.request.urlopen(req, timeout=15) as resp:
              data = json.load(resp)

          msg = data.get("message") or data.get("commit", {}).get("message", "")
          first_line = msg.splitlines()[0] if msg else ""
          print(f"commit {sha}: {first_line}")

          if "[ci-pass]" not in msg:
              print("marker absent; failing")
              sys.exit(1)

          print("marker present; passing")
          PY
"#;

/// Builds the Forgejo seed commits applied to a freshly created demo
/// repository's default branch: the marker CI workflow (`runs-on: host`, the
/// `[ci-pass]` commit-message gate) followed by a sentinel commit whose message
/// carries the `[ci-pass]` marker so the just-installed workflow passes on the
/// default branch.
///
/// Both mutate repository history, so a caller provisioning onto a repo that
/// owns its own CI (`--existing-repo`) must NOT apply them; the orchestration in
/// `temper-provision` already skips a plan's `seed_commits` when `existing_repo`
/// is set.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_workflow_uses_commit_message_marker() {
        assert!(CI_WORKFLOW.contains("runs-on: host"));
        assert!(CI_WORKFLOW.contains(CI_PASS_MARKER));
        assert!(CI_WORKFLOW.contains("GITHUB_SHA"));
        assert!(CI_WORKFLOW.contains("/git/commits/"));
        assert!(!CI_WORKFLOW.contains("github.event.head_commit.message"));
    }

    #[test]
    fn ci_workflow_yaml_is_indented_not_flush_left() {
        // Regression for lesson 0013: a `\<newline>`-continued string literal
        // strips the YAML indentation, producing a flush-left, invalid workflow
        // that Forgejo silently fails to detect. The mapping keys MUST keep
        // their leading spaces.
        assert!(
            CI_WORKFLOW.contains("\n  build:\n"),
            "`build:` must be indented under `jobs:`"
        );
        assert!(
            CI_WORKFLOW.contains("\n    runs-on: host\n"),
            "`runs-on:` must be indented under `build:`"
        );
        assert!(
            CI_WORKFLOW.contains("\n    steps:\n"),
            "`steps:` must be indented under `build:`"
        );
        // No job-level key should appear flush-left (column 0).
        assert!(!CI_WORKFLOW.contains("\nbuild:\n"));
        assert!(!CI_WORKFLOW.contains("\nruns-on:"));
    }

    #[test]
    fn ci_seed_commits_install_workflow_then_sentinel() {
        let commits = ci_seed_commits("main");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].path, WORKFLOW_PATH);
        assert_eq!(commits[0].contents, CI_WORKFLOW.as_bytes());
        assert_eq!(commits[0].branch, "main");
        assert_eq!(commits[1].path, ".temper-ci/main.txt");
        assert!(commits[1].message.contains(CI_PASS_MARKER));
    }

    #[test]
    fn ci_sentinel_commit_sanitizes_branch_path() {
        let commit = ci_sentinel_commit("feature/x");
        assert_eq!(commit.path, ".temper-ci/feature-x.txt");
        assert_eq!(commit.branch, "feature/x");
        assert!(commit.message.contains(CI_PASS_MARKER));
    }
}
