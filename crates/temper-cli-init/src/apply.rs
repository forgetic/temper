// SPDX-License-Identifier: MPL-2.0

//! `temper apply` — run the forge-side provisioning step for an init bundle.
//!
//! `temper init` writes a local deployment bundle by default. This module owns
//! the explicit follow-up mutation path: load that bundle, derive the same
//! [`ProvisionRequest`](crate::ProvisionRequest) `init --apply` would have used,
//! run the injected provisioner, and persist the minted forge credentials.

use std::process::ExitCode;

use temper_cli_common::{
    EX_USAGE, EnvMap, LoadOptions, PathResolver, Prompter, TerminalPrompter, resolve_targets,
};
use temper_config::{Config, Credentials, ResolveOptions};

use crate::provisioner::{ForgejoProvisioner, ProvisionOutcome, ProvisionRequest, Provisioner};
use crate::{InitError, write};

/// `temper apply [OPTIONS]` usage.
pub const APPLY_USAGE: &str = "\
Apply a temper deployment bundle to the forge.

Loads config.toml + credentials.toml, provisions the configured Forgejo repo,
users, labels, and webhook, then updates credentials.toml with minted tokens.

Usage: temper [GLOBAL OPTIONS] apply [OPTIONS]

Options:
  --existing-repo         Provision onto a repo that already exists
  --yes                   Skip the provisioning confirmation
  -h, --help              Print help";

/// Everything `temper apply` needs beyond the loaded bundle.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Where to read `config.toml` and read/write `credentials.toml`.
    pub options: LoadOptions,
    /// Provision onto a repo that must already exist (`--existing-repo`).
    pub existing_repo: bool,
    /// Skip the confirmation before forge-side mutations.
    pub yes: bool,
    /// Environment snapshot used for path expansion and for systemd
    /// `CREDENTIALS_DIRECTORY` credentials discovery.
    pub env: EnvMap,
    /// Base directories used to resolve default config locations.
    pub paths: PathResolver,
}

#[derive(Debug, Clone, Default)]
struct ParsedApplyArgs {
    help: bool,
    options: LoadOptions,
    existing_repo: bool,
    yes: bool,
}

/// The unified binary's `temper apply` entry point.
pub fn apply_main(args: Vec<String>, env: &EnvMap, paths: &PathResolver) -> ExitCode {
    apply_main_with_options(args, env, paths, LoadOptions::default())
}

pub fn apply_main_with_options(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let parsed = match parse_apply_args(args, options) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper apply: {error}\n\n{APPLY_USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{APPLY_USAGE}");
        return ExitCode::SUCCESS;
    }

    let opts = ApplyOptions {
        options: parsed.options,
        existing_repo: parsed.existing_repo,
        yes: parsed.yes,
        env: env.clone(),
        paths: paths.clone(),
    };
    let mut prompter = TerminalPrompter::stdio();
    let mut provisioner = ForgejoProvisioner;
    match run_apply(&mut prompter, &mut provisioner, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper apply: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_apply_args(args: Vec<String>, options: LoadOptions) -> Result<ParsedApplyArgs, String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedApplyArgs {
            help: true,
            options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedApplyArgs {
        options,
        ..Default::default()
    };
    for arg in args {
        match arg.as_str() {
            "--existing-repo" => parsed.existing_repo = true,
            "--yes" => parsed.yes = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}

/// Loads an init bundle, provisions the forge, and updates credentials.
pub fn run_apply(
    p: &mut dyn Prompter,
    provisioner: &mut dyn Provisioner,
    opts: &ApplyOptions,
) -> Result<(), InitError> {
    let bundle = load_apply_bundle(opts)?;
    if !opts.yes
        && !p.confirm(
            &format!(
                "Provision {}/{} on {} and register {}?",
                bundle.request.owner,
                bundle.request.name,
                bundle.request.base_url,
                bundle.request.webhook_url
            ),
            false,
        )?
    {
        p.note("Skipped forge provisioning at operator confirmation.");
        return Ok(());
    }

    let outcome = provisioner
        .provision(&bundle.request)
        .map_err(InitError::Provision)?;
    let mut credentials = bundle.credentials;
    merge_provisioned_credentials(&mut credentials, &bundle.admin_key, &outcome);
    temper_config::write_credentials(&credentials, &bundle.credentials_path, true)
        .map_err(|error| InitError::Write(error.to_string()))?;

    p.note(&format!(
        "Provisioned {}/{} with {} role(s) and the `{}` automation bot.",
        outcome.provisioned.owner,
        outcome.provisioned.name,
        outcome.provisioned.roles.len(),
        outcome.provisioned.automation.user,
    ));
    p.note(&format!(
        "Updated {} (chmod 600)",
        bundle.credentials_path.display()
    ));
    p.note("Now run `temper serve standalone` to start the engine, worker, and agent.");
    Ok(())
}

struct ApplyBundle {
    request: ProvisionRequest,
    credentials: Credentials,
    credentials_path: std::path::PathBuf,
    admin_key: String,
}

fn load_apply_bundle(opts: &ApplyOptions) -> Result<ApplyBundle, InitError> {
    let targets =
        resolve_targets(&opts.options, &opts.env, &opts.paths).map_err(InitError::Path)?;
    let config = Config::load(&targets.config)
        .map_err(|error| InitError::Path(format!("load {}: {error}", targets.config.display())))?;
    let credentials = Credentials::load(&targets.credentials).map_err(|error| {
        InitError::Path(format!("load {}: {error}", targets.credentials.display()))
    })?;
    let resolved = {
        let mut resolve_options = targets
            .config
            .parent()
            .map(ResolveOptions::from_config_base_dir)
            .unwrap_or_default();
        // Local init bundles may reference the forge token that `temper apply`
        // is about to mint. Resolve non-strictly here so apply can converge the
        // bundle; normal runtime/check paths keep strict secret validation after apply.
        resolve_options.validate_secret_references = false;
        temper_config::resolve_with_options(&config, &credentials, &opts.env, &resolve_options)
            .map_err(|error| InitError::Path(format!("resolve deployment: {error}")))?
    };
    if let Some(path) = &resolved.engine.workflow_file {
        temper_reference_delivery::load_workflow(path)
            .map_err(|error| InitError::Unsupported(error.to_string()))?;
    }

    let base_url = resolved.forge.url.clone().ok_or_else(|| {
        InitError::Unsupported("temper apply requires `[forge] url` in config.toml".to_string())
    })?;
    if resolved.engine.repos.len() != 1 {
        return Err(InitError::Unsupported(format!(
            "temper apply requires exactly one `[engine] repos` entry, found {}",
            resolved.engine.repos.len()
        )));
    }
    let repo = &resolved.engine.repos[0];
    let webhook_secret_file = resolved.engine.webhook_secret_file.clone().ok_or_else(|| {
        InitError::Unsupported(
            "temper apply requires `[engine] webhook_secret_file` in config.toml".to_string(),
        )
    })?;

    let admin_key = non_empty(config.forge.admin.as_deref()).ok_or_else(|| {
        InitError::Unsupported("temper apply requires `[forge] admin` in config.toml".to_string())
    })?;
    let admin = credentials.forge.users.get(&admin_key).ok_or_else(|| {
        InitError::Unsupported(format!(
            "temper apply requires `[forge.users.{admin_key}]` in credentials.toml"
        ))
    })?;
    let admin_user = non_empty(admin.user.as_deref()).unwrap_or_else(|| admin_key.clone());
    let admin_password = non_empty(admin.password.as_deref()).ok_or_else(|| {
        InitError::Unsupported(format!(
            "temper apply requires a password under `[forge.users.{admin_key}]` in credentials.toml"
        ))
    })?;

    let request = ProvisionRequest {
        base_url,
        admin_user,
        admin_password,
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        webhook_url: format!("http://{}/forgejo/webhook", resolved.engine.bind),
        webhook_secret_file,
        workflow_path: resolved.engine.workflow_file.clone(),
        existing_repo: opts.existing_repo,
    };

    Ok(ApplyBundle {
        request,
        credentials,
        credentials_path: targets.credentials,
        admin_key,
    })
}

fn merge_provisioned_credentials(
    credentials: &mut Credentials,
    admin_key: &str,
    outcome: &ProvisionOutcome,
) {
    for (key, user) in write::provisioned_role_and_bot_users(outcome) {
        credentials.forge.users.insert(key, user);
    }
    let admin = credentials
        .forge
        .users
        .entry(admin_key.to_string())
        .or_default();
    admin.token = Some(outcome.admin_token.clone());
    write::add_forge_engine_token_secret(credentials, outcome.admin_token.clone());
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use temper_cli_common::{LoadOptions, ScriptedPrompter};
    use temper_forge::RepositoryId;
    use temper_provision::{Provisioned, RoleIdentity};
    use temper_workflow::RoleId;

    use super::*;

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
            Ok(ProvisionOutcome {
                provisioned: Provisioned {
                    owner: request.owner.clone(),
                    name: request.name.clone(),
                    repository: RepositoryId::new(format!("{}/{}", request.owner, request.name)),
                    roles,
                    automation: identity("bot"),
                },
                admin_token: "admin-rest-token".to_string(),
            })
        }
    }

    #[test]
    fn run_apply_loads_init_bundle_and_updates_credentials() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let credentials_path = dir.path().join("credentials.toml");
        let webhook_secret_path = dir.path().join("webhook-secret");
        std::fs::write(&webhook_secret_path, "secret").expect("webhook secret");
        std::fs::write(
            &config_path,
            format!(
                "schema_version = 1\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\nadmin = \"root\"\nci_user = \"bot\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [\"acme/service\"]\nroles = [\"architect\", \"engineer\"]\nwebhook_secret_file = \"{}\"\n",
                webhook_secret_path.display()
            ),
        )
        .expect("config");
        std::fs::write(
            &credentials_path,
            "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n\n[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"sk-key\"\n",
        )
        .expect("credentials");

        let opts = ApplyOptions {
            options: LoadOptions {
                config: Some(config_path.clone()),
                credentials: Some(credentials_path.clone()),
            },
            yes: true,
            ..Default::default()
        };
        let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
        let mut provisioner = StubProvisioner { seen: None };

        run_apply(&mut prompter, &mut provisioner, &opts).expect("apply succeeds");

        let seen = provisioner.seen.expect("provisioner called");
        assert_eq!(seen.base_url, "http://forge.local:3000");
        assert_eq!(seen.admin_user, "root");
        assert_eq!(seen.admin_password, "admin-pass");
        assert_eq!(seen.owner, "acme");
        assert_eq!(seen.name, "service");
        assert_eq!(seen.webhook_url, "http://127.0.0.1:38100/forgejo/webhook");
        assert_eq!(seen.webhook_secret_file, webhook_secret_path);

        let creds = std::fs::read_to_string(&credentials_path).expect("credentials updated");
        assert!(creds.contains("admin-rest-token"), "{creds}");
        assert!(creds.contains("token-architect"), "{creds}");
        assert!(creds.contains("token-engineer"), "{creds}");
        assert!(creds.contains("token-bot"), "{creds}");
        assert!(
            creds.contains("sk-key"),
            "provider secret preserved: {creds}"
        );
    }

    #[test]
    fn run_apply_loads_target_init_bundle_with_relative_paths_and_mints_named_forge_secret() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let credentials_path = dir.path().join("credentials.toml");
        let webhook_secret_path = dir.path().join("webhook-secret");
        let workflow_path = dir.path().join("workflow.yaml");
        std::fs::write(&webhook_secret_path, "secret").expect("webhook secret");
        let workflow_spec: temper_workflow::RawWorkflowSpec =
            serde_json::from_str(temper_reference_delivery::basic_delivery_workflow_json())
                .expect("basic workflow parses");
        let workflow_yaml = serde_yaml::to_string(&workflow_spec).expect("workflow yaml");
        std::fs::write(&workflow_path, workflow_yaml).expect("workflow");
        std::fs::write(
            &config_path,
            "schema_version = 1\n\n[deployment]\ntopology = \"standalone\"\n\n[workflow]\nfile = \"workflow.yaml\"\n\n[paths]\nworkspace_dir = \"workspace\"\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\nadmin = \"root\"\nci_user = \"bot\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nworkflow = \"workflow.yaml\"\nrepos = [\"acme/service\"]\nroles = [\"architect\", \"engineer\"]\nforge_token = \"forge-engine-token\"\nwebhook_secret = \"webhook-secret\"\nwebhook_secret_file = \"webhook-secret\"\n\n[worker]\nworkspace = \"workspace\"\n\n[[worker.pools]]\nname = \"local\"\nroles = [\"architect\", \"engineer\"]\nrepos = [\"acme/service\"]\nmax_concurrent_jobs = 1\nworker_token = \"worker-local-token\"\n",
        )
        .expect("config");
        std::fs::write(
            &credentials_path,
            "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n\n[secrets.webhook-secret]\nkind = \"webhook-secret\"\nsecret = \"secret\"\n\n[secrets.worker-local-token]\nkind = \"worker-token\"\ntoken = \"worker-secret\"\n",
        )
        .expect("credentials");

        let opts = ApplyOptions {
            options: LoadOptions {
                config: Some(config_path.clone()),
                credentials: Some(credentials_path.clone()),
            },
            yes: true,
            ..Default::default()
        };
        let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
        let mut provisioner = StubProvisioner { seen: None };

        run_apply(&mut prompter, &mut provisioner, &opts).expect("target apply succeeds");

        let seen = provisioner.seen.expect("provisioner called");
        assert_eq!(seen.webhook_secret_file, webhook_secret_path);
        assert_eq!(seen.workflow_path.as_deref(), Some(workflow_path.as_path()));
        let creds = std::fs::read_to_string(&credentials_path).expect("credentials updated");
        assert!(creds.contains("[secrets.forge-engine-token]"), "{creds}");
        assert!(creds.contains("token = \"admin-rest-token\""), "{creds}");
    }

    #[test]
    fn run_apply_confirmation_can_skip_provisioning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let credentials_path = dir.path().join("credentials.toml");
        let webhook_secret_path = dir.path().join("webhook-secret");
        std::fs::write(&webhook_secret_path, "secret").expect("webhook secret");
        std::fs::write(
            &config_path,
            format!(
                "schema_version = 1\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\nadmin = \"root\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [\"acme/service\"]\nroles = [\"architect\"]\nwebhook_secret_file = \"{}\"\n",
                webhook_secret_path.display()
            ),
        )
        .expect("config");
        std::fs::write(
            &credentials_path,
            "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n",
        )
        .expect("credentials");

        let opts = ApplyOptions {
            options: LoadOptions {
                config: Some(config_path),
                credentials: Some(credentials_path.clone()),
            },
            yes: false,
            ..Default::default()
        };
        let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
        prompter.confirmations.push_back(false);
        let mut provisioner = StubProvisioner { seen: None };

        run_apply(&mut prompter, &mut provisioner, &opts).expect("apply skip succeeds");

        assert!(provisioner.seen.is_none());
        let creds = std::fs::read_to_string(&credentials_path).expect("credentials unchanged");
        assert!(!creds.contains("admin-rest-token"), "{creds}");
        assert!(
            prompter
                .notes
                .iter()
                .any(|note| note.contains("Skipped forge provisioning")),
            "notes: {:?}",
            prompter.notes
        );
    }

    #[test]
    fn run_apply_resolves_relative_workflow_and_secret_against_config_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("bundle");
        std::fs::create_dir_all(bundle.join("flows")).expect("create flows");
        std::fs::create_dir_all(bundle.join("secrets")).expect("create secrets");
        let workflow_spec: temper_workflow::RawWorkflowSpec =
            serde_json::from_str(temper_reference_delivery::basic_delivery_workflow_json())
                .expect("basic workflow parses");
        let workflow_yaml = serde_yaml::to_string(&workflow_spec).expect("workflow yaml");
        std::fs::write(bundle.join("flows/workflow.yaml"), workflow_yaml).expect("workflow");
        std::fs::write(bundle.join("secrets/webhook-secret"), "secret").expect("webhook secret");
        std::fs::write(
            bundle.join("config.toml"),
            "schema_version = 1\n\
             [workflow]\n\
             file = \"flows/workflow.yaml\"\n\
             [forge]\n\
             type = \"forgejo\"\n\
             url = \"http://forge.local:3000\"\n\
             admin = \"root\"\n\
             [engine]\n\
             bind = \"127.0.0.1:38100\"\n\
             repos = [\"acme/service\"]\n\
             roles = [\"architect\", \"engineer\"]\n\
             webhook_secret_file = \"secrets/webhook-secret\"\n",
        )
        .expect("config");
        std::fs::write(
            bundle.join("credentials.toml"),
            "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n",
        )
        .expect("credentials");

        let opts = ApplyOptions {
            options: LoadOptions {
                config: Some(bundle.clone()),
                credentials: None,
            },
            yes: true,
            ..Default::default()
        };
        let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
        let mut provisioner = StubProvisioner { seen: None };

        run_apply(&mut prompter, &mut provisioner, &opts).expect("apply succeeds");

        let seen = provisioner.seen.expect("provisioner called");
        assert_eq!(
            seen.workflow_path.as_deref(),
            Some(bundle.join("flows/workflow.yaml").as_path())
        );
        assert_eq!(
            seen.webhook_secret_file,
            bundle.join("secrets/webhook-secret")
        );
    }

    #[test]
    fn run_apply_reports_workflow_static_validation_before_provisioning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let credentials_path = dir.path().join("credentials.toml");
        let workflow_path = dir.path().join("invalid-workflow.json");
        std::fs::write(
            &workflow_path,
            r#"{
                "name": "invalid",
                "roles": [{"id": "engineer", "queues": ["missing_queue"]}]
            }"#,
        )
        .expect("workflow");
        std::fs::write(
            &config_path,
            "schema_version = 1\n\
             [workflow]\n\
             file = \"invalid-workflow.json\"\n\
             [forge]\n\
             type = \"forgejo\"\n\
             url = \"http://forge.local:3000\"\n\
             admin = \"root\"\n\
             [engine]\n\
             bind = \"127.0.0.1:38100\"\n\
             repos = [\"acme/service\"]\n\
             roles = [\"engineer\"]\n\
             webhook_secret_file = \"webhook-secret\"\n",
        )
        .expect("config");
        std::fs::write(
            &credentials_path,
            "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n",
        )
        .expect("credentials");

        let opts = ApplyOptions {
            options: LoadOptions {
                config: Some(config_path),
                credentials: Some(credentials_path),
            },
            yes: true,
            ..Default::default()
        };
        let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
        let mut provisioner = StubProvisioner { seen: None };

        let err = run_apply(&mut prompter, &mut provisioner, &opts)
            .expect_err("invalid workflow should fail before provisioning");

        let message = err.to_string();
        assert!(message.contains("failed validation"), "{message}");
        assert!(
            message.contains("undeclared queue `missing_queue`"),
            "{message}"
        );
        assert!(provisioner.seen.is_none());
    }

    #[test]
    fn parse_accepts_yes_existing_repo_and_global_options() {
        let parsed = parse_apply_args(
            vec!["--yes".to_string(), "--existing-repo".to_string()],
            LoadOptions {
                config: Some("bundle".into()),
                credentials: None,
            },
        )
        .expect("parse");

        assert_eq!(parsed.options.config, Some("bundle".into()));
        assert!(parsed.yes);
        assert!(parsed.existing_repo);
    }

    #[test]
    fn parse_rejects_local_config_flag() {
        let err = parse_apply_args(
            vec!["--config".to_string(), "bundle".to_string()],
            LoadOptions::default(),
        )
        .expect_err("--config is global-only");

        assert!(err.contains("--config"), "{err}");
    }
}
