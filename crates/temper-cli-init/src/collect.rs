// SPDX-License-Identifier: MPL-2.0

//! Step 1 of `temper init`: gather answers interactively. Prompts ONLY — no
//! disk and no network I/O, so the step is fully exercised by a scripted
//! prompter.

use temper_cli_common::Prompter;

use crate::InitError;

/// The default webhook bind/advertise address written into `[engine] bind` and
/// registered on the forge.
pub const DEFAULT_WEBHOOK_ADDR: &str = "http://localhost:8314";

/// The only workflow `temper init` offers today (the embedded basic-delivery
/// reference shape).
pub const WORKFLOW_BASIC_DELIVERY: &str = "basic-delivery";

/// The default repository `owner/name` provisioned and driven. Matches the
/// reference-delivery repo input so the embedded workflow's roles line up.
pub const DEFAULT_REPO: &str = "acme/service";

/// Provider name and auth shape are fixed for the first-run flow.
pub const PROVIDER_DEEPSEEK: &str = "deepseek";

/// The collected, non-secret + secret answers from the interactive flow.
///
/// `repos`/`roles` are not asked: they are derived from the embedded workflow's
/// queue-subscribing roles and the default repo at [`build_artifacts`] time.
#[derive(Debug, Clone)]
pub struct Answers {
    /// The Forgejo base URL (`[forge] url`).
    pub forge_url: String,
    /// The selected workflow name (only `basic-delivery` today).
    pub workflow: String,
    /// The webhook bind/advertise address (`[engine] bind`).
    pub webhook_addr: String,
    /// Forge admin login.
    pub admin_user: String,
    /// Forge admin password (used to mint the admin REST token). **Secret.**
    pub admin_password: String,
    /// DeepSeek API key. **Secret.**
    pub provider_key: String,
    /// Repository owner (derived from [`DEFAULT_REPO`]).
    pub repo_owner: String,
    /// Repository name (derived from [`DEFAULT_REPO`]).
    pub repo_name: String,
}

impl Answers {
    /// The `owner/name` the deployment drives.
    pub fn repo_path(&self) -> String {
        format!("{}/{}", self.repo_owner, self.repo_name)
    }

    /// The webhook URL the daemon registers (the bind address + the engine's
    /// webhook route).
    pub fn webhook_url(&self) -> String {
        format!("{}/webhook", self.webhook_addr.trim_end_matches('/'))
    }
}

/// Asks the operator the five questions + two secret prompts, validating the
/// forge URL and rejecting hosted GitHub (unsupported today).
pub fn collect_answers(p: &mut dyn Prompter) -> Result<Answers, InitError> {
    // Q1 — Forge URL. The default literal `github` is a deliberate nudge: a real
    // GitHub deployment is not supported yet, so an unchanged default (or any
    // non-URL) is rejected with guidance to enter a Forgejo URL.
    let forge_url = p.ask("Forge URL", Some("github"))?;
    if !looks_like_url(&forge_url) {
        return Err(InitError::Unsupported(
            "hosted GitHub is not supported yet; enter a Forgejo URL \
             (e.g. http://localhost:3000)"
                .to_string(),
        ));
    }

    // Q2 — Workflow. Only basic-delivery (embedded) today.
    let workflow = p.ask("Workflow", Some(WORKFLOW_BASIC_DELIVERY))?;
    if workflow != WORKFLOW_BASIC_DELIVERY {
        return Err(InitError::Unsupported(format!(
            "unknown workflow `{workflow}`; only `{WORKFLOW_BASIC_DELIVERY}` is supported"
        )));
    }

    // Q3 — Daemon webhook address.
    let webhook_addr = p.ask("Daemon webhook address", Some(DEFAULT_WEBHOOK_ADDR))?;

    // Q4 — Forge admin user + password.
    let admin_user = p.ask("Forge admin user", None)?;
    if admin_user.is_empty() {
        return Err(InitError::Unsupported(
            "forge admin user is required".to_string(),
        ));
    }
    let admin_password = p.ask_secret("Forge admin password")?;

    // Q5 — DeepSeek API key (provider + auth shape are fixed).
    let provider_key = p.ask_secret("DeepSeek API key")?;

    let (repo_owner, repo_name) = split_repo(DEFAULT_REPO);

    Ok(Answers {
        forge_url,
        workflow,
        webhook_addr,
        admin_user,
        admin_password,
        provider_key,
        repo_owner,
        repo_name,
    })
}

/// A lightweight URL check: an `http`/`https` scheme with a non-empty host.
/// Avoids pulling a URL-parsing dependency into the lightest CLI crate.
fn looks_like_url(value: &str) -> bool {
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"));
    match rest {
        Some(host) => !host.is_empty(),
        None => false,
    }
}

/// Splits `owner/name`, falling back to the whole string as the name when there
/// is no slash.
fn split_repo(value: &str) -> (String, String) {
    match value.split_once('/') {
        Some((owner, name)) => (owner.to_string(), name.to_string()),
        None => (String::new(), value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_cli_common::ScriptedPrompter;

    fn full_answers() -> ScriptedPrompter {
        ScriptedPrompter::new([
            "http://localhost:3000".to_string(), // forge URL
            "".to_string(),                      // workflow (default)
            "".to_string(),                      // webhook (default)
            "root".to_string(),                  // admin user
            "admin-pw".to_string(),              // admin password (secret)
            "sk-deepseek".to_string(),           // provider key (secret)
        ])
    }

    #[test]
    fn collects_with_defaults() {
        let mut p = full_answers();
        let a = collect_answers(&mut p).expect("collect");
        assert_eq!(a.forge_url, "http://localhost:3000");
        assert_eq!(a.workflow, WORKFLOW_BASIC_DELIVERY);
        assert_eq!(a.webhook_addr, DEFAULT_WEBHOOK_ADDR);
        assert_eq!(a.admin_user, "root");
        assert_eq!(a.admin_password, "admin-pw");
        assert_eq!(a.provider_key, "sk-deepseek");
        assert_eq!(a.repo_owner, "acme");
        assert_eq!(a.repo_name, "service");
        // The webhook URL is the *daemon's* address (where the forge POSTs
        // events), not the forge URL.
        assert_eq!(a.webhook_url(), "http://localhost:8314/webhook");
    }

    #[test]
    fn rejects_github_default() {
        // Unchanged `github` default → not a URL → Unsupported.
        let mut p = ScriptedPrompter::new(["".to_string()]);
        let err = collect_answers(&mut p).expect_err("github rejected");
        assert!(matches!(err, InitError::Unsupported(_)), "{err}");
    }

    #[test]
    fn rejects_unknown_workflow() {
        let mut p = ScriptedPrompter::new([
            "http://localhost:3000".to_string(),
            "fancy-delivery".to_string(),
        ]);
        let err = collect_answers(&mut p).expect_err("unknown workflow rejected");
        assert!(matches!(err, InitError::Unsupported(_)), "{err}");
    }
}
