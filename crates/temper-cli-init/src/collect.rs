// SPDX-License-Identifier: MPL-2.0

//! Step 1 of `temper init`: gather answers interactively. Prompts ONLY — no
//! disk and no network I/O, so the step is fully exercised by a scripted
//! prompter.

use temper_cli_common::Prompter;

use crate::{InitError, InitOverrides, RepoSelection};

/// The default webhook bind/advertise address written into `[engine] bind` and
/// registered on the forge.
pub const DEFAULT_WEBHOOK_ADDR: &str = "http://127.0.0.1:8314";

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
/// `roles` are not asked: they are derived from the embedded workflow's
/// queue-subscribing roles at [`build_artifacts`] time. The managed repo uses
/// [`DEFAULT_REPO`] unless `--repo` supplies an override.
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
    /// LLM provider profile (only [`PROVIDER_DEEPSEEK`] today).
    pub provider: String,
    /// DeepSeek API key. **Secret.**
    pub provider_key: String,
    /// LLM provider base URL override (for `[agent.providers.<name>].url`).
    /// Only set via `--provider-url`; the interactive flow never prompts for it.
    pub provider_url: Option<String>,
    /// Repository owner (derived from [`DEFAULT_REPO`] or `--repo`).
    pub repo_owner: String,
    /// Repository name (derived from [`DEFAULT_REPO`] or `--repo`).
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
        format!(
            "{}/forgejo/webhook",
            self.webhook_addr.trim_end_matches('/')
        )
    }
}

/// Asks the operator the five questions + two secret prompts, validating the
/// forge URL and rejecting hosted GitHub (unsupported today). `--forge` skips
/// only the forge URL prompt; the rest of the provisioning flow stays intact.
///
/// When `non_interactive` is true, prompts are skipped entirely and all values
/// are taken from the overrides (or errors when required values are missing).
pub fn collect_answers(
    p: &mut dyn Prompter,
    overrides: &InitOverrides,
    non_interactive: bool,
) -> Result<Answers, InitError> {
    if non_interactive {
        return collect_non_interactive(overrides);
    }

    // Q1 — Forge URL. The default literal `github` is a deliberate nudge: a real
    // GitHub deployment is not supported yet, so an unchanged default (or any
    // non-URL) is rejected with guidance to enter a Forgejo URL.
    let forge_url = match &overrides.forge_url {
        Some(value) => value.clone(),
        None => p.ask("Forge URL", Some("github"))?,
    };
    validate_forge_url(&forge_url)?;

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
    let provider = provider_from_override(overrides.provider.as_deref())?;
    let provider_key = p.ask_secret("DeepSeek API key")?;

    let repo = overrides.repo.clone().unwrap_or_else(default_repo);

    Ok(Answers {
        forge_url,
        workflow,
        webhook_addr,
        admin_user,
        admin_password,
        provider,
        provider_key,
        provider_url: overrides.provider_url.clone(),
        repo_owner: repo.owner,
        repo_name: repo.name,
    })
}

/// Non-interactive path: all values come from overrides, or an error is
/// returned with the name of the missing flag / env var (never a secret value).
fn collect_non_interactive(overrides: &InitOverrides) -> Result<Answers, InitError> {
    let forge_url = overrides.forge_url.clone().ok_or_else(|| {
        InitError::Unsupported("--non-interactive: forge URL is required; pass --forge".to_string())
    })?;
    validate_forge_url(&forge_url)?;

    let workflow = WORKFLOW_BASIC_DELIVERY.to_string();
    let webhook_addr = DEFAULT_WEBHOOK_ADDR.to_string();

    let admin_user = overrides.admin_user.clone().ok_or_else(|| {
        InitError::Unsupported(
            "--non-interactive: admin user is required; pass --admin-user".to_string(),
        )
    })?;
    let admin_password = overrides.admin_password.clone().ok_or_else(|| {
        InitError::Unsupported(
            "--non-interactive: admin password is required; set TEMPER_INIT_ADMIN_PASSWORD"
                .to_string(),
        )
    })?;

    let provider = provider_from_override(overrides.provider.as_deref())?;
    let provider_key = overrides.provider_key.clone().ok_or_else(|| {
        InitError::Unsupported(
            "--non-interactive: provider key is required; set TEMPER_INIT_PROVIDER_KEY".to_string(),
        )
    })?;

    let repo = overrides.repo.clone().unwrap_or_else(default_repo);

    Ok(Answers {
        forge_url,
        workflow,
        webhook_addr,
        admin_user,
        admin_password,
        provider,
        provider_key,
        provider_url: overrides.provider_url.clone(),
        repo_owner: repo.owner,
        repo_name: repo.name,
    })
}

fn validate_forge_url(forge_url: &str) -> Result<(), InitError> {
    if looks_like_url(forge_url) {
        Ok(())
    } else {
        Err(InitError::Unsupported(
            "hosted GitHub is not supported yet; enter a Forgejo URL \
             (e.g. http://localhost:3000)"
                .to_string(),
        ))
    }
}

fn provider_from_override(provider: Option<&str>) -> Result<String, InitError> {
    match provider.unwrap_or(PROVIDER_DEEPSEEK) {
        PROVIDER_DEEPSEEK => Ok(PROVIDER_DEEPSEEK.to_string()),
        other => Err(InitError::Unsupported(format!(
            "unsupported provider `{other}`; only `{PROVIDER_DEEPSEEK}` is supported"
        ))),
    }
}

fn default_repo() -> RepoSelection {
    RepoSelection::parse(DEFAULT_REPO).expect("DEFAULT_REPO is a valid owner/name repo")
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
        let a = collect_answers(&mut p, &InitOverrides::default(), false).expect("collect");
        assert_eq!(a.forge_url, "http://localhost:3000");
        assert_eq!(a.workflow, WORKFLOW_BASIC_DELIVERY);
        assert_eq!(a.webhook_addr, DEFAULT_WEBHOOK_ADDR);
        assert_eq!(a.admin_user, "root");
        assert_eq!(a.admin_password, "admin-pw");
        assert_eq!(a.provider, PROVIDER_DEEPSEEK);
        assert_eq!(a.provider_key, "sk-deepseek");
        assert_eq!(a.repo_owner, "acme");
        assert_eq!(a.repo_name, "service");
        // The webhook URL is the *daemon's* address (where the forge POSTs
        // events), not the forge URL.
        assert_eq!(a.webhook_url(), "http://127.0.0.1:8314/forgejo/webhook");
    }

    #[test]
    fn rejects_github_default() {
        // Unchanged `github` default → not a URL → Unsupported.
        let mut p = ScriptedPrompter::new(["".to_string()]);
        let err =
            collect_answers(&mut p, &InitOverrides::default(), false).expect_err("github rejected");
        assert!(matches!(err, InitError::Unsupported(_)), "{err}");
    }

    #[test]
    fn rejects_unknown_workflow() {
        let mut p = ScriptedPrompter::new([
            "http://localhost:3000".to_string(),
            "fancy-delivery".to_string(),
        ]);
        let err = collect_answers(&mut p, &InitOverrides::default(), false)
            .expect_err("unknown workflow rejected");
        assert!(matches!(err, InitError::Unsupported(_)), "{err}");
    }

    #[test]
    fn forge_override_skips_forge_prompt_and_is_validated() {
        let mut p = ScriptedPrompter::new([
            "".to_string(),            // workflow (default)
            "".to_string(),            // webhook (default)
            "root".to_string(),        // admin user
            "admin-pw".to_string(),    // admin password
            "sk-deepseek".to_string(), // provider key
        ]);
        let overrides = InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            ..Default::default()
        };

        let a = collect_answers(&mut p, &overrides, false).expect("collect");

        assert_eq!(a.forge_url, "http://forge.local:3000");
        assert!(p.answers.is_empty(), "all non-forge prompts consumed");
    }

    #[test]
    fn forge_override_uses_same_validation_as_prompted_forge() {
        let mut p = ScriptedPrompter::new(Vec::<String>::new());
        let overrides = InitOverrides {
            forge_url: Some("github".to_string()),
            ..Default::default()
        };

        let err = collect_answers(&mut p, &overrides, false).expect_err("github rejected");

        assert!(matches!(err, InitError::Unsupported(_)), "{err}");
    }
}
