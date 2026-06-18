// SPDX-License-Identifier: MPL-2.0

//! Full-flow unit test of `temper init`'s testable core, `run_init`, EXCEPT the
//! live forge call.
//!
//! The provisioning step is the only one that touches a network; `run_init`
//! takes a `&mut dyn Provisioner`, so this test passes a [`StubProvisioner`]
//! that returns a canned [`Provisioned`] without a forge. Everything else —
//! collecting answers (including defaults-on-empty), building the documents,
//! writing config.toml / workflow.json / webhook-secret / credentials.toml, and
//! the 0600 mode on the two secret files — runs for real against a temp dir.
//!
//! Issue #183's e2e reuses this exact seam: `run_init` + `ScriptedPrompter` +
//! `InitOptions`, but with a real `ForgejoProvisioner` instead of the stub.

use std::collections::BTreeMap;

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{
    InitOptions, InitOverrides, InitTopology, ProvisionOutcome, ProvisionRequest, Provisioner,
    RepoSelection, run_init,
};
use temper_forge::RepositoryId;
use temper_provision::{Provisioned, RoleIdentity};
use temper_workflow::RoleId;

/// Returns a canned `Provisioned` for `acme/service` with two role identities
/// (architect, engineer) + a `bot` automation identity, and records the request
/// it was handed so the test can assert the wiring.
struct StubProvisioner {
    seen: Option<ProvisionRequest>,
}

impl Provisioner for StubProvisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String> {
        self.seen = Some(request.clone());
        let identity = |user: &str| RoleIdentity {
            user: user.to_string(),
            email: format!("{user}@example.invalid"),
            token: format!("token-{user}"),
            password: format!("pw-{user}"),
        };
        let mut roles = BTreeMap::new();
        roles.insert(RoleId::new("architect"), identity("architect"));
        roles.insert(RoleId::new("engineer"), identity("engineer"));
        let provisioned = Provisioned {
            owner: request.owner.clone(),
            name: request.name.clone(),
            repository: RepositoryId::new(format!("{}/{}", request.owner, request.name)),
            roles,
            automation: identity("bot"),
        };
        Ok(ProvisionOutcome {
            provisioned,
            // The admin token the live ForgejoProvisioner would mint from the
            // Q4 password; the stub returns a deterministic stand-in.
            admin_token: "admin-rest-token".to_string(),
        })
    }
}

#[test]
fn run_init_collects_writes_and_provisions_offline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");

    // Scripted answers in prompt order. Empty strings exercise defaults-on-empty
    // for the workflow and webhook-address questions.
    let mut prompter = ScriptedPrompter::new([
        "http://localhost:3000".to_string(), // Q1 forge URL
        "".to_string(),                      // Q2 workflow (default basic-delivery)
        "".to_string(),                      // Q3 webhook addr (default)
        "root".to_string(),                  // Q4 admin user
        "admin-pass".to_string(),            // Q4 admin password (secret)
        "sk-deepseek-xyz".to_string(),       // Q5 DeepSeek API key (secret)
    ]);

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        force: false,
        existing_repo: false,
        workspace: None,
        ..Default::default()
    };

    let mut provisioner = StubProvisioner { seen: None };
    run_init(&mut prompter, &mut provisioner, &opts).expect("run_init succeeds offline");

    // ── config.toml ──────────────────────────────────────────────────────────
    let config = std::fs::read_to_string(&config_path).expect("config.toml written");
    assert!(
        config.contains("url = \"http://localhost:3000\""),
        "{config}"
    );
    assert!(config.contains("type = \"forgejo\""), "{config}");
    assert!(config.contains("admin = \"root\""), "{config}");
    // CI reader points at the automation bot.
    assert!(config.contains("ci_user = \"bot\""), "{config}");
    // Repo + roles derived from the embedded basic-delivery workflow.
    assert!(config.contains("acme/service"), "{config}");
    assert!(config.contains("architect"), "{config}");
    assert!(config.contains("engineer"), "{config}");
    // Webhook bind address scheme-stripped to host:port.
    assert!(config.contains("bind = \"127.0.0.1:8314\""), "{config}");
    // Provider profile + webhook secret + workflow file wired.
    assert!(config.contains("provider = \"deepseek\""), "{config}");
    assert!(config.contains("workflow.json"), "{config}");
    assert!(config.contains("webhook-secret"), "{config}");

    // ── workflow.json ─────────────────────────────────────────────────────────
    let workflow_path = dir.path().join("workflow.json");
    let workflow = std::fs::read_to_string(&workflow_path).expect("workflow.json written");
    assert_eq!(
        workflow,
        temper_reference_delivery::basic_delivery_workflow_json(),
        "workflow.json is the embedded basic-delivery bytes verbatim"
    );

    // ── credentials.toml ──────────────────────────────────────────────────────
    let creds = std::fs::read_to_string(&credentials_path).expect("credentials.toml written");
    // Admin identity: token is the minted admin REST token, password is Q4.
    assert!(creds.contains("token = \"admin-rest-token\""), "{creds}");
    assert!(creds.contains("password = \"admin-pass\""), "{creds}");
    // Minted role identities folded in.
    assert!(creds.contains("token-architect"), "{creds}");
    assert!(creds.contains("token-engineer"), "{creds}");
    // Automation bot identity.
    assert!(creds.contains("token-bot"), "{creds}");
    // Provider key under [agent.providers.deepseek] as an api-key.
    assert!(creds.contains("sk-deepseek-xyz"), "{creds}");
    assert!(creds.contains("api-key"), "{creds}");

    // ── 0600 on the two secret files ─────────────────────────────────────────
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let secret_path = dir.path().join("webhook-secret");
        for path in [&credentials_path, &secret_path] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o600,
                "{} should be 0600, got {mode:o}",
                path.display()
            );
        }
    }

    // ── the provisioner was handed the right request ─────────────────────────
    let seen = provisioner.seen.expect("provisioner was called");
    assert_eq!(seen.base_url, "http://localhost:3000");
    assert_eq!(seen.admin_user, "root");
    assert_eq!(seen.admin_password, "admin-pass");
    assert_eq!(seen.owner, "acme");
    assert_eq!(seen.name, "service");
    assert_eq!(seen.webhook_url, "http://127.0.0.1:8314/forgejo/webhook");
    assert!(!seen.existing_repo);
    // The webhook secret file the adapter reads back is the one we wrote.
    assert!(seen.webhook_secret_file.ends_with("webhook-secret"));

    // ── a final summary was emitted ──────────────────────────────────────────
    assert!(
        prompter
            .notes
            .iter()
            .any(|n| n.contains("temper serve standalone")),
        "summary should point at `temper serve standalone`: {:?}",
        prompter.notes
    );
}

#[test]
fn run_init_uses_local_dev_flag_overrides_in_artifacts_and_provisioning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");

    // `--forge` skips the first prompt, so this starts at workflow.
    let mut prompter = ScriptedPrompter::new([
        "".to_string(),                     // Q2 workflow (default basic-delivery)
        "".to_string(),                     // Q3 webhook addr (default)
        "root".to_string(),                 // Q4 admin user
        "admin-pass".to_string(),           // Q4 admin password (secret)
        "sk-deepseek-override".to_string(), // Q5 DeepSeek API key (secret)
    ]);

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        topology: InitTopology::Standalone,
        overrides: InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            repo: Some(RepoSelection {
                owner: "widgets".to_string(),
                name: "service".to_string(),
            }),
            provider: Some("deepseek".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut provisioner = StubProvisioner { seen: None };
    run_init(&mut prompter, &mut provisioner, &opts).expect("run_init succeeds offline");

    let config = std::fs::read_to_string(&config_path).expect("config.toml written");
    assert!(
        config.contains("url = \"http://forge.local:3000\""),
        "{config}"
    );
    assert!(config.contains("widgets/service"), "{config}");
    assert!(!config.contains("acme/service"), "{config}");
    assert!(config.contains("provider = \"deepseek\""), "{config}");

    let creds = std::fs::read_to_string(&credentials_path).expect("credentials.toml written");
    assert!(creds.contains("sk-deepseek-override"), "{creds}");

    let seen = provisioner.seen.expect("provisioner was called");
    assert_eq!(seen.base_url, "http://forge.local:3000");
    assert_eq!(seen.owner, "widgets");
    assert_eq!(seen.name, "service");
    assert!(
        prompter.answers.is_empty(),
        "forge prompt should be skipped"
    );
}

#[test]
fn run_init_refuses_to_clobber_without_force() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");
    // Pre-create config.toml so preflight trips.
    std::fs::write(&config_path, "schema_version = 1\n").expect("seed config");

    let mut prompter = ScriptedPrompter::new([
        "http://localhost:3000".to_string(),
        "".to_string(),
        "".to_string(),
        "root".to_string(),
        "admin-pass".to_string(),
        "sk-deepseek-xyz".to_string(),
    ]);
    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path),
            credentials: Some(credentials_path),
        },
        force: false,
        existing_repo: false,
        workspace: None,
        ..Default::default()
    };
    let mut provisioner = StubProvisioner { seen: None };
    let err = run_init(&mut prompter, &mut provisioner, &opts).expect_err("clobber refused");
    assert!(err.to_string().contains("already exist"), "{err}");
    // Preflight runs before provisioning, so the stub was never called.
    assert!(
        provisioner.seen.is_none(),
        "provisioner must not run on clobber"
    );
}

#[test]
fn non_interactive_with_all_overrides_succeeds_without_consuming_answers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");

    // NO scripted answers needed — everything comes from overrides.
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        non_interactive: true,
        overrides: InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            repo: Some(RepoSelection {
                owner: "widgets".to_string(),
                name: "svc".to_string(),
            }),
            admin_user: Some("root".to_string()),
            admin_password: Some("admin-pass".to_string()),
            provider_key: Some("sk-key".to_string()),
            provider: Some("deepseek".to_string()),
        },
        ..Default::default()
    };

    let mut provisioner = StubProvisioner { seen: None };
    run_init(&mut prompter, &mut provisioner, &opts).expect("non-interactive succeeds");

    // Assert: no prompts consumed.
    assert!(prompter.answers.is_empty(), "no prompts should fire");

    // Assert: config reflects overrides.
    let config = std::fs::read_to_string(&config_path).expect("config.toml");
    assert!(config.contains("url = \"http://forge.local:3000\""));
    assert!(config.contains("widgets/svc"));

    // Assert: provisioner was called with the right data.
    let seen = provisioner.seen.expect("provisioner called");
    assert_eq!(seen.admin_user, "root");
    assert_eq!(seen.admin_password, "admin-pass");

    // Assert: credentials file contains the provider key.
    let creds = std::fs::read_to_string(&credentials_path).expect("credentials.toml");
    assert!(creds.contains("sk-key"));
}

#[test]
fn non_interactive_missing_admin_user_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(dir.path().join("config.toml")),
            credentials: Some(dir.path().join("credentials.toml")),
        },
        non_interactive: true,
        overrides: InitOverrides {
            forge_url: Some("http://forge.local:3000".to_string()),
            admin_password: Some("pw".to_string()),
            provider_key: Some("key".to_string()),
            // admin_user intentionally missing
            ..Default::default()
        },
        ..Default::default()
    };

    let mut provisioner = StubProvisioner { seen: None };
    let err = run_init(&mut prompter, &mut provisioner, &opts)
        .expect_err("missing admin user should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("admin user"),
        "error should mention admin user: {msg}"
    );
    assert!(
        msg.contains("--admin-user"),
        "error should mention --admin-user flag: {msg}"
    );
    // Provisioner must not have been called.
    assert!(provisioner.seen.is_none());
}

#[test]
fn non_interactive_flag_off_ignores_env_overrides() {
    // When --non-interactive is NOT set, env-based overrides in InitOverrides
    // are ignored and the interactive flow fires prompts as usual.
    let dir = tempfile::tempdir().expect("tempdir");

    // Scripted answers are the source of truth (env overrides ignored).
    let mut prompter = ScriptedPrompter::new([
        "http://localhost:3000".to_string(),
        "".to_string(),
        "".to_string(),
        "interactive-admin".to_string(),
        "interactive-pw".to_string(),
        "sk-interactive".to_string(),
    ]);

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(dir.path().join("config.toml")),
            credentials: Some(dir.path().join("credentials.toml")),
        },
        non_interactive: false, // NOT non-interactive
        overrides: InitOverrides {
            // These would be populated from env in main(), but should be
            // ignored since non_interactive is false.
            admin_user: Some("env-user".to_string()),
            admin_password: Some("env-pw".to_string()),
            provider_key: Some("env-key".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut provisioner = StubProvisioner { seen: None };
    run_init(&mut prompter, &mut provisioner, &opts).expect("run_init succeeds");

    // Assert: all six scripted answers were consumed.
    assert!(prompter.answers.is_empty(), "all prompts should fire");

    // Assert: the INTERACTIVE values were used, not the env overrides.
    let seen = provisioner.seen.expect("provisioner called");
    assert_eq!(seen.admin_user, "interactive-admin");
    assert_eq!(seen.admin_password, "interactive-pw");

    let creds =
        std::fs::read_to_string(dir.path().join("credentials.toml")).expect("credentials.toml");
    assert!(creds.contains("sk-interactive"));
    assert!(!creds.contains("env-key"));
}
