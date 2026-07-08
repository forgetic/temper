// SPDX-License-Identifier: MPL-2.0

//! Step 1 of `temper init`: gather answers interactively. Prompts ONLY — no
//! disk and no network I/O, so the step is fully exercised by a scripted
//! prompter.

use temper_cli_common::Prompter;

use crate::{InitError, InitOverrides, RepoSelection};

/// The default webhook bind/advertise address written into `[engine] bind` and
/// registered on the forge.
pub const DEFAULT_WEBHOOK_ADDR: &str = "http://127.0.0.1:8314";

/// The default workflow `temper init` offers (the embedded basic-delivery
/// reference shape).
pub const WORKFLOW_BASIC_DELIVERY: &str = "basic-delivery";

/// The richer bundled workflow with an explicit reviewer gate.
pub const WORKFLOW_REFERENCE_DELIVERY: &str = "reference-delivery";

/// The default repository `owner/name` provisioned and driven. Matches the
/// reference-delivery repo input so the embedded workflow's roles line up.
pub const DEFAULT_REPO: &str = "acme/service";

/// Provider name and auth shape for the default first-run flow.
pub const PROVIDER_DEEPSEEK: &str = "deepseek";

/// Provider selection for bundles that intentionally omit agent credentials.
pub const PROVIDER_NONE: &str = "none";

/// The collected, non-secret + secret answers from the interactive flow.
///
/// `roles` are not asked: they are derived from the embedded workflow's
/// queue-subscribing roles at [`build_artifacts`] time. The managed repo uses
/// [`DEFAULT_REPO`] unless `--repo` supplies an override.
#[derive(Debug, Clone)]
pub struct Answers {
    /// The Forgejo base URL (`[forge] url`).
    pub forge_url: String,
    /// The selected workflow: a builtin name or a JSON/YAML file path.
    pub workflow: String,
    /// The webhook bind/advertise address (`[engine] bind`).
    pub webhook_addr: String,
    /// Forge admin login.
    pub admin_user: String,
    /// Forge admin password (used to mint the admin REST token). **Secret.**
    pub admin_password: String,
    /// LLM provider profile (`deepseek`, or `none` to omit provider credentials).
    pub provider: String,
    /// DeepSeek API key. **Secret.** Omitted when `provider = "none"`.
    pub provider_key: Option<String>,
    /// LLM provider base URL override (for `[agent.providers.<name>].url`).
    /// Only set via `--provider-url`; the interactive flow never prompts for it.
    pub provider_url: Option<String>,
    /// Repository owner (derived from [`DEFAULT_REPO`] or `--repo`).
    pub repo_owner: String,
    /// Repository name (derived from [`DEFAULT_REPO`] or `--repo`).
    pub repo_name: String,
    /// Additional repository selections from repeated `--repo` flags.
    pub extra_repos: Vec<RepoSelection>,
}

impl Answers {
    /// The `owner/name` the deployment drives.
    pub fn repo_path(&self) -> String {
        format!("{}/{}", self.repo_owner, self.repo_name)
    }

    /// Every `owner/name` path the generated deployment should drive.
    pub fn repo_paths(&self) -> Vec<String> {
        std::iter::once(self.repo_path())
            .chain(self.extra_repos.iter().map(RepoSelection::path))
            .collect()
    }

    /// True when init should write provider/runtime credential wiring.
    pub fn provider_enabled(&self) -> bool {
        self.provider != PROVIDER_NONE
    }

    /// The webhook URL the daemon registers (the bind address + the engine's
    /// webhook route).
    pub fn webhook_url(&self) -> String {
        let base = self.webhook_addr.trim_end_matches('/');
        let base = if base.starts_with("http://") || base.starts_with("https://") {
            base.to_string()
        } else {
            format!("http://{base}")
        };
        format!("{base}/forgejo/webhook")
    }
}

/// Asks the operator for any missing non-secret answers plus required secret
/// prompts (admin password and, unless `--provider none`, provider key),
/// validating the forge URL and rejecting hosted GitHub (unsupported today).
/// Prompt-overriding flags such as `--forge` and `--admin-user` skip only their
/// matching prompt; the rest of the provisioning flow stays intact.
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

    // Q2 — Workflow. Builtins are embedded; path-like answers are loaded later
    // when artifacts are built, keeping this step prompt-only.
    let workflow = match &overrides.workflow {
        Some(value) => value.clone(),
        None => p.ask("Workflow", Some(WORKFLOW_BASIC_DELIVERY))?,
    };
    validate_workflow_selection(&workflow)?;

    // Q3 — Daemon webhook address.
    let webhook_addr = match &overrides.bind {
        Some(value) => value.clone(),
        None => p.ask("Daemon webhook address", Some(DEFAULT_WEBHOOK_ADDR))?,
    };

    // Q4 — Forge admin user + password. `--admin-user` skips only the username
    // prompt; the password remains a secret prompt in interactive mode.
    let admin_user = match &overrides.admin_user {
        Some(value) => value.clone(),
        None => p.ask("Forge admin user", None)?,
    };
    if admin_user.is_empty() {
        return Err(InitError::Unsupported(
            "forge admin user is required".to_string(),
        ));
    }
    let admin_password = p.ask_secret("Forge admin password")?;

    // Q5 — Provider key. `--provider none` intentionally skips provider
    // credential collection for bundles that only need forge/runtime wiring.
    let provider = provider_from_override(overrides.provider.as_deref())?;
    if provider == PROVIDER_NONE && overrides.provider_url.is_some() {
        return Err(InitError::Unsupported(
            "--provider-url requires an active provider; omit it with --provider none".to_string(),
        ));
    }
    let provider_key = if provider == PROVIDER_NONE {
        None
    } else {
        Some(p.ask_secret("DeepSeek API key")?)
    };

    let (repo, extra_repos) = selected_repos(overrides);

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
        extra_repos,
    })
}

/// Non-interactive path: all values come from overrides, or an error is
/// returned with the name of the missing flag / env var (never a secret value).
fn collect_non_interactive(overrides: &InitOverrides) -> Result<Answers, InitError> {
    let forge_url = overrides.forge_url.clone().ok_or_else(|| {
        InitError::Unsupported("--non-interactive: forge URL is required; pass --forge".to_string())
    })?;
    validate_forge_url(&forge_url)?;

    let workflow = overrides
        .workflow
        .clone()
        .unwrap_or_else(|| WORKFLOW_BASIC_DELIVERY.to_string());
    validate_workflow_selection(&workflow)?;
    let webhook_addr = overrides
        .bind
        .clone()
        .unwrap_or_else(|| DEFAULT_WEBHOOK_ADDR.to_string());

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
    if provider == PROVIDER_NONE && overrides.provider_url.is_some() {
        return Err(InitError::Unsupported(
            "--provider-url requires an active provider; omit it with --provider none".to_string(),
        ));
    }
    let provider_key = if provider == PROVIDER_NONE {
        None
    } else {
        Some(overrides.provider_key.clone().ok_or_else(|| {
            InitError::Unsupported(
                "--non-interactive: provider key is required; set TEMPER_INIT_PROVIDER_KEY"
                    .to_string(),
            )
        })?)
    };

    let (repo, extra_repos) = selected_repos(overrides);

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
        extra_repos,
    })
}

fn validate_workflow_selection(workflow: &str) -> Result<(), InitError> {
    if workflow == WORKFLOW_BASIC_DELIVERY || workflow == WORKFLOW_REFERENCE_DELIVERY {
        return Ok(());
    }
    if workflow.contains('/')
        || workflow.contains('\\')
        || workflow.ends_with(".json")
        || workflow.ends_with(".yaml")
        || workflow.ends_with(".yml")
    {
        return Ok(());
    }
    Err(InitError::Unsupported(format!(
        "unknown workflow `{workflow}`; use `{WORKFLOW_BASIC_DELIVERY}`, \
         `{WORKFLOW_REFERENCE_DELIVERY}`, or a workflow JSON/YAML file path"
    )))
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
        PROVIDER_NONE => Ok(PROVIDER_NONE.to_string()),
        other => Err(InitError::Unsupported(format!(
            "unsupported provider `{other}`; expected `{PROVIDER_DEEPSEEK}` or `{PROVIDER_NONE}`"
        ))),
    }
}

fn selected_repos(overrides: &InitOverrides) -> (RepoSelection, Vec<RepoSelection>) {
    (
        overrides.repo.clone().unwrap_or_else(default_repo),
        overrides.extra_repos.clone(),
    )
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
        assert_eq!(a.provider_key.as_deref(), Some("sk-deepseek"));
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
    fn bind_override_skips_webhook_prompt() {
        let mut p = ScriptedPrompter::new([
            "".to_string(),            // workflow (default)
            "root".to_string(),        // admin user
            "admin-pw".to_string(),    // admin password
            "sk-deepseek".to_string(), // provider key
        ]);
        let overrides = InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            bind: Some("127.0.0.1:38100".to_string()),
            ..Default::default()
        };

        let a = collect_answers(&mut p, &overrides, false).expect("collect");

        assert_eq!(a.webhook_addr, "127.0.0.1:38100");
        assert_eq!(a.webhook_url(), "http://127.0.0.1:38100/forgejo/webhook");
        assert!(p.answers.is_empty(), "webhook prompt should be skipped");
    }

    #[test]
    fn admin_user_override_skips_admin_prompt_and_consumes_one_fewer_answer() {
        let mut p = ScriptedPrompter::new([
            "http://localhost:3000".to_string(), // forge URL
            "".to_string(),                      // workflow (default)
            "".to_string(),                      // webhook (default)
            "admin-pw".to_string(),              // admin password (secret)
            "sk-deepseek".to_string(),           // provider key (secret)
        ]);
        let overrides = InitOverrides {
            admin_user: Some("flag-admin".to_string()),
            ..Default::default()
        };

        let a = collect_answers(&mut p, &overrides, false).expect("collect");

        assert_eq!(a.admin_user, "flag-admin");
        assert_eq!(a.admin_password, "admin-pw");
        assert_eq!(a.provider_key.as_deref(), Some("sk-deepseek"));
        assert!(p.answers.is_empty(), "admin-user prompt should be skipped");
    }

    #[test]
    fn empty_admin_user_override_errors_in_interactive_mode() {
        let mut p = ScriptedPrompter::new(Vec::<String>::new());
        let overrides = InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            workflow: Some(WORKFLOW_BASIC_DELIVERY.to_string()),
            bind: Some(DEFAULT_WEBHOOK_ADDR.to_string()),
            admin_user: Some(String::new()),
            ..Default::default()
        };

        let err = collect_answers(&mut p, &overrides, false)
            .expect_err("empty --admin-user should fail interactively");

        assert!(matches!(&err, InitError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("admin user is required"), "{err}");
        assert!(
            p.answers.is_empty(),
            "empty admin override should not prompt"
        );
    }

    #[test]
    fn non_interactive_bind_overrides_default_webhook_addr() {
        let mut p = ScriptedPrompter::new(Vec::<String>::new());
        let overrides = InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            bind: Some("127.0.0.1:38100".to_string()),
            admin_user: Some("root".to_string()),
            admin_password: Some("admin-pw".to_string()),
            provider_key: Some("sk-deepseek".to_string()),
            ..Default::default()
        };

        let a = collect_answers(&mut p, &overrides, true).expect("collect");

        assert_eq!(a.webhook_addr, "127.0.0.1:38100");
        assert_eq!(a.webhook_url(), "http://127.0.0.1:38100/forgejo/webhook");
        assert_eq!(a.admin_user, "root");
        assert_eq!(a.admin_password, "admin-pw");
        assert_eq!(a.provider_key.as_deref(), Some("sk-deepseek"));
        assert!(p.answers.is_empty(), "non-interactive should not prompt");
    }

    #[test]
    fn provider_none_skips_provider_secret_prompt() {
        let mut p = ScriptedPrompter::new([
            "http://localhost:3000".to_string(),
            "".to_string(),
            "".to_string(),
            "root".to_string(),
            "admin-pw".to_string(),
        ]);
        let overrides = InitOverrides {
            provider: Some(PROVIDER_NONE.to_string()),
            ..Default::default()
        };

        let a = collect_answers(&mut p, &overrides, false).expect("collect");

        assert_eq!(a.provider, PROVIDER_NONE);
        assert_eq!(a.provider_key, None);
        assert!(
            p.answers.is_empty(),
            "provider key prompt should be skipped"
        );
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
