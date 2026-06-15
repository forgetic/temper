//! Forgejo-specific provisioning constants and the adapter's CLI-facing
//! options.
//!
//! The host-neutral provisioning types (`Provisioned`, `RoleIdentity`,
//! `IntakeIssueSeed`, `ProvisionError`, and the `BOT_USER` constant) now live in
//! `temper-provision` and are re-exported here for back-compat. This module owns
//! only the genuinely Forgejo-specific pieces: the bundled CI workflow YAML, its
//! repository path, and the demo intake defaults.

use temper_forge::AccessScope;

pub const DEFAULT_INTAKE_TITLE: &str = "Add a configurable greeting to the service banner";
pub const DEFAULT_INTAKE_BODY: &str = "As an operator I want the service banner to show a \
configurable greeting so I can tell environments apart at a glance.\n\n\
Acceptance: a `BANNER_GREETING` setting whose value is printed on startup, \
defaulting to the current text when unset.";

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

/// Options that tune [`provision_world`](super::provision_world) away from its
/// throwaway-repo defaults.
///
/// Both fields default to today's behavior, so `ProvisionOptions::default()`
/// leaves the throwaway `reference-delivery` / `basic-delivery` flows unchanged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProvisionOptions {
    /// Provision onto a repo that must already exist: require the repo up front
    /// (erroring if absent), and skip the marker CI commit and the CI sentinel
    /// commit so the repo's own `.forgejo/workflows/ci.yml` and history are
    /// never touched. Labels, the webhook, and `enable_actions` still apply.
    pub existing_repo: bool,
    /// How role users and the `bot` are granted access to the repo.
    pub access: AccessScope,
}
