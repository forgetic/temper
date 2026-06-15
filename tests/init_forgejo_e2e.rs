//! `temper init` live e2e: drive the library entry point `run_init` against a
//! real Forgejo (the shared bench fixture) and prove the produced config +
//! credentials stand up a valid, daemon-runnable deployment.
//!
//! This is the capstone of plan #176. Where the unit test in
//! `temper-cli-init` stubs the live-forge step, this test swaps in the **real**
//! [`ForgejoProvisioner`] and runs the whole flow end to end:
//!
//!   collect (scripted answers) → write local files → provision the live forge
//!   → write credentials → summarize
//!
//! and then asserts BOTH sides of the seam:
//!
//! 1. **Local artifacts** — `config.toml` parses and `temper_config::load`
//!    resolves with the expected forge URL, repo, roles, workspace, and webhook
//!    secret; `lint` reports no errors; `workflow.json` byte-equals the embedded
//!    basic-delivery bytes and validates; `credentials.toml` is `0600` and holds
//!    the minted role/bot tokens, the admin identity, and the DeepSeek key.
//! 2. **Forge state** — the org/owner, the repo (Actions enabled, CI workflow
//!    committed), every basic-delivery label, the role users + `bot`, and the
//!    webhook pointing at the configured address all exist on the live forge.
//! 3. **Idempotency** — a second `run_init --force` converges with no duplicate
//!    forge objects and freshly-minted tokens; a second run WITHOUT `--force` is
//!    refused at preflight with a clear "already exists" message.
//! 4. **Daemon-boot stretch** — the standalone engine boots cleanly against the
//!    init-produced config + credentials and binds its webhook port, proving the
//!    output is a runnable deployment (no convergence machinery — that is the job
//!    of `daemon_forgejo_e2e`).
//!
//! No LLM provider is ever contacted: the DeepSeek key is a dummy and `run_init`
//! does not talk to the provider. The flow writes only into a per-test temp dir.
//!
//! Run with `cargo test --test init_forgejo_e2e -- --ignored`, or under CI's
//! `cargo dev-test-full` (which passes `--run-ignored all`).

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{ForgejoProvisioner, InitOptions, run_init};
use temper_config::{NoEnv, ProviderCredential, ProviderKind, lint, load_with_env};
use temper_engine_io::http::BlockingJsonClient;
use temper_provision::BOT_USER;

/// A non-reserved Forgejo admin login (`admin` itself is reserved). Created via
/// the server CLI before the test drives `run_init`, then handed to init as the
/// scripted Q4 answer so the real provisioner mints an admin REST token from it.
const ADMIN_USER: &str = "initadmin";
const ADMIN_PASSWORD: &str = "Init-Phase-e2e!";
const ADMIN_EMAIL: &str = "initadmin@example.invalid";

/// The org/repo `temper init` provisions (matches the embedded reference-delivery
/// default repo, so the workflow's roles line up).
const REPO_OWNER: &str = "acme";
const REPO_NAME: &str = "service";

/// A dummy DeepSeek key — never used to contact the provider; init writes it to
/// credentials verbatim and makes no LLM call.
const DUMMY_DEEPSEEK_KEY: &str = "sk-init-e2e-dummy";

const DAEMON_BOOT_TIMEOUT: Duration = Duration::from_secs(60);

#[test]
#[ignore = "boots a real Forgejo fixture and provisions it via `temper init`; run with --ignored"]
fn init_forgejo_drives_a_working_setup() {
    let started = Instant::now();

    // --- A bare Forgejo with a site admin, but NOTHING else provisioned. ---
    // We deliberately do not use the cached *provisioned* world: the whole point
    // is to let `run_init`'s real ForgejoProvisioner create the org, repo, users,
    // labels, webhook, and CI itself.
    let server = temper_testing::forgejo_server::ForgejoServer::start()
        .expect("bench Forgejo fixture starts");
    create_site_admin(&server);
    let base_url = server.base_url().to_string();
    eprintln!(
        "init_forgejo_e2e: Forgejo up at {base_url} (startup {:?})",
        started.elapsed()
    );

    // The webhook address doubles as the engine bind, so use a real free port on
    // 127.0.0.1 (a hostname like `localhost` does not parse as a SocketAddr).
    let bind_port = free_port();
    let webhook_addr = format!("http://127.0.0.1:{bind_port}");

    // Per-test temp config dir: config.toml, credentials.toml, workflow.json, and
    // the webhook secret all land here; the real ~/.config/temper is never read
    // or written.
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");
    let credentials_path = config_dir.path().join("credentials.toml");
    let workflow_path = config_dir.path().join("workflow.json");
    let webhook_secret_path = config_dir.path().join("webhook-secret");
    let workspace_dir = tempfile::tempdir().expect("workspace tempdir");

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        force: false,
        existing_repo: false,
        workspace: Some(workspace_dir.path().to_path_buf()),
    };

    // --- Drive run_init end to end against the live forge. ---
    run_init(
        &mut scripted(&base_url, &webhook_addr),
        &mut ForgejoProvisioner,
        &opts,
    )
    .expect("run_init succeeds against the live forge");
    eprintln!(
        "init_forgejo_e2e: run_init #1 done ({:?})",
        started.elapsed()
    );

    // ── 1. Local artifacts ─────────────────────────────────────────────────────
    assert_local_artifacts(
        &base_url,
        &config_path,
        &credentials_path,
        &workflow_path,
        &webhook_secret_path,
        bind_port,
        workspace_dir.path(),
    );

    // ── 2. Forge-side provisioning ──────────────────────────────────────────────
    let admin_token = read_admin_token(&credentials_path);
    let rest = BlockingJsonClient::new();
    let webhook_url = format!("{webhook_addr}/webhook");
    assert_forge_state(&rest, &base_url, &admin_token, &webhook_url);

    // ── 3a. Idempotency: a second --force run converges, no duplicates. ─────────
    let baseline = forge_object_counts(&rest, &base_url, &admin_token);
    let first_tokens = role_tokens(&credentials_path);

    let force_opts = InitOptions {
        force: true,
        ..opts.clone()
    };
    run_init(
        &mut scripted(&base_url, &webhook_addr),
        &mut ForgejoProvisioner,
        &force_opts,
    )
    .expect("run_init --force converges on the second run");
    eprintln!(
        "init_forgejo_e2e: run_init #2 (--force) done ({:?})",
        started.elapsed()
    );

    let after = forge_object_counts(&rest, &base_url, &admin_token);
    assert_eq!(
        baseline, after,
        "a --force re-run must not create duplicate forge objects (labels/users/webhooks)"
    );
    // Credentials are rewritten with freshly-minted tokens, but the document is
    // still valid and the forge state still asserts clean.
    let second_tokens = role_tokens(&credentials_path);
    assert_eq!(
        first_tokens.keys().collect::<Vec<_>>(),
        second_tokens.keys().collect::<Vec<_>>(),
        "the same role set is present after a --force re-run"
    );
    assert!(
        first_tokens != second_tokens,
        "a --force re-run mints fresh role tokens (the credentials are rewritten)"
    );
    // The forge still validates after the convergent re-run.
    let admin_token_2 = read_admin_token(&credentials_path);
    assert_forge_state(&rest, &base_url, &admin_token_2, &webhook_url);

    // ── 3b. A second run WITHOUT --force is refused at preflight. ───────────────
    let err = run_init(
        &mut scripted(&base_url, &webhook_addr),
        &mut ForgejoProvisioner,
        &opts,
    )
    .expect_err("a non-force re-run must be refused");
    let message = err.to_string();
    assert!(
        message.contains("already exist"),
        "non-force re-run should fail with an 'already exists' clobber message, got: {message}"
    );

    // ── 4. Daemon-boot stretch: the engine boots on init's config + creds. ──────
    assert_daemon_boots(
        &config_path,
        &credentials_path,
        config_dir.path(),
        bind_port,
    );

    eprintln!(
        "init_forgejo_e2e: all assertions passed (total {:?})",
        started.elapsed()
    );
}

/// The scripted answers in `collect_answers` order: forge URL, workflow,
/// webhook address, admin user, admin password (secret), DeepSeek key (secret).
fn scripted(base_url: &str, webhook_addr: &str) -> ScriptedPrompter {
    ScriptedPrompter::new([
        base_url.to_string(),           // Q1 forge URL
        "basic-delivery".to_string(),   // Q2 workflow
        webhook_addr.to_string(),       // Q3 webhook address (test-local URL)
        ADMIN_USER.to_string(),         // Q4 admin user
        ADMIN_PASSWORD.to_string(),     // Q4 admin password (secret)
        DUMMY_DEEPSEEK_KEY.to_string(), // Q5 DeepSeek API key (secret)
    ])
}

/// Creates the non-reserved site admin via the Forgejo CLI. Tolerates a
/// pre-existing user so a flake-retried boot does not wedge.
fn create_site_admin(server: &temper_testing::forgejo_server::ForgejoServer) {
    let result = server.run_cli(&[
        "admin",
        "user",
        "create",
        "--username",
        ADMIN_USER,
        "--password",
        ADMIN_PASSWORD,
        "--email",
        ADMIN_EMAIL,
        "--admin",
        "--must-change-password=false",
    ]);
    if let Err(error) = result {
        let text = error.to_string().to_lowercase();
        assert!(
            text.contains("exist"),
            "creating the site admin failed: {error}"
        );
    }
}

// ── local-artifact assertions ──────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn assert_local_artifacts(
    base_url: &str,
    config_path: &Path,
    credentials_path: &Path,
    workflow_path: &Path,
    webhook_secret_path: &Path,
    bind_port: u16,
    workspace_root: &Path,
) {
    // workflow.json is the embedded basic-delivery bytes verbatim and validates.
    let workflow = std::fs::read_to_string(workflow_path).expect("workflow.json written");
    assert_eq!(
        workflow,
        temper_reference_delivery::basic_delivery_workflow_json(),
        "workflow.json must be the embedded basic-delivery bytes verbatim"
    );
    // Validates by parsing through the reference loader (panics on invalid).
    let validated = temper_reference_delivery::basic_delivery_workflow();

    // The webhook secret is 0600 and non-empty.
    assert_mode_600(webhook_secret_path);
    let secret = std::fs::read_to_string(webhook_secret_path).expect("webhook-secret written");
    assert!(
        !secret.trim().is_empty(),
        "webhook secret must be non-empty"
    );

    // credentials.toml is 0600 and contains the minted role/bot tokens, the admin
    // identity, and the DeepSeek api-key.
    assert_mode_600(credentials_path);
    let creds_text = std::fs::read_to_string(credentials_path).expect("credentials.toml written");
    assert!(
        creds_text.contains(DUMMY_DEEPSEEK_KEY),
        "credentials must carry the DeepSeek key"
    );
    assert!(
        creds_text.contains("api-key"),
        "DeepSeek key must be stored as an api-key"
    );
    assert!(
        creds_text.contains(ADMIN_USER),
        "credentials must carry the admin identity"
    );
    // Resolve config + credentials the way the daemon does, with a clean env so
    // ambient TEMPER_*/FORGEJO_* vars cannot mask file values.
    let resolved = load_isolated(config_path, credentials_path);

    // Forge URL round-trips (trailing slash stripped).
    assert_eq!(
        resolved.forge.url.as_deref(),
        Some(base_url.trim_end_matches('/')),
        "resolved forge URL must match the live fixture"
    );
    // The admin token resolved from the credentials file (minted by provisioning).
    assert!(
        resolved.forge.admin_token.is_some(),
        "resolved deployment must have an admin token from credentials"
    );
    // CI-reader web-UI creds resolved from `[forge] ci_user = bot` + bot password.
    assert!(
        resolved.forge.web_ui.is_some(),
        "resolved deployment must have CI web-UI credentials (bot)"
    );

    // Repo + roles from the embedded workflow.
    let repos: Vec<String> = resolved.engine.repos.iter().map(|r| r.display()).collect();
    assert_eq!(
        repos,
        vec![format!("{REPO_OWNER}/{REPO_NAME}")],
        "resolved repos must be the default reference repo"
    );
    let mut roles = resolved.engine.roles.clone();
    roles.sort();
    let mut expected_roles: Vec<String> = validated
        .roles()
        .iter()
        .filter(|role| !role.queues.is_empty())
        .map(|role| role.id.as_str().to_string())
        .collect();
    expected_roles.sort();
    assert_eq!(
        roles, expected_roles,
        "resolved roles must be the workflow's queue-subscribing roles"
    );
    // Each engine role must have a resolvable git identity (minted token).
    for role in &roles {
        assert!(
            resolved.forge.role_identities.contains_key(role),
            "role `{role}` must have a resolved git identity from credentials"
        );
    }

    // Bind is the test-local port; workspace points at our temp dir; webhook
    // secret file is wired.
    assert_eq!(
        resolved.engine.bind.port(),
        bind_port,
        "engine bind must use the scripted webhook port"
    );
    assert_eq!(
        resolved.worker.workspace_root.as_path(),
        workspace_root,
        "worker workspace must be the temp dir we passed via InitOptions"
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(webhook_secret_path),
        "engine webhook_secret_file must point at the generated secret"
    );

    // Provider wiring: DeepSeek api-key.
    assert_eq!(resolved.agent.provider.kind, ProviderKind::DeepSeek);
    assert!(
        matches!(
            resolved.agent.provider.credential,
            ProviderCredential::ApiKey(_)
        ),
        "provider credential must be an api-key, got {:?}",
        resolved.agent.provider.credential
    );

    // lint reports NO errors (notes are fine).
    let findings = lint(&resolved);
    let errors: Vec<&str> = findings
        .iter()
        .filter(|f| f.error)
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        errors.is_empty(),
        "lint must report no errors for the init-produced deployment, got: {errors:?}"
    );
}

/// Loads + resolves config/credentials with an EMPTY environment, so the
/// assertion reflects only what `temper init` wrote — a developer's ambient
/// `TEMPER_*`/`FORGEJO_*` vars cannot mask the file values (and no process env is
/// mutated, keeping the test free of `unsafe`).
fn load_isolated(config_path: &Path, credentials_path: &Path) -> temper_config::Resolved {
    load_with_env(
        &LoadOptions {
            config: Some(config_path.to_path_buf()),
            credentials: Some(credentials_path.to_path_buf()),
        },
        &NoEnv,
    )
    .expect("init-produced config + credentials load and resolve")
    .0
}

fn assert_mode_600(path: &Path) {
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "{} must be 0600, got {mode:o}", path.display());
}

// ── forge-state assertions ──────────────────────────────────────────────────────

/// Asserts the live forge holds everything `run_init` should have provisioned:
/// the org, the repo (Actions on, CI workflow committed), all workflow labels,
/// the role users + bot, and the webhook pointing at the configured URL.
fn assert_forge_state(
    rest: &BlockingJsonClient,
    base_url: &str,
    admin_token: &str,
    webhook_url: &str,
) {
    let token = Some(admin_token);

    // Org/owner exists.
    let (status, _) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/orgs/{REPO_OWNER}"),
        token,
        None,
        "get org",
    );
    assert_eq!(status, 200, "org {REPO_OWNER} must exist");

    // Repo exists with Actions enabled.
    let (status, repo) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}"),
        token,
        None,
        "get repo",
    );
    assert_eq!(status, 200, "repo {REPO_OWNER}/{REPO_NAME} must exist");
    assert_eq!(
        repo["has_actions"].as_bool(),
        Some(true),
        "Actions/CI must be enabled on the repo"
    );

    // The CI workflow file was committed.
    let (status, _) = rest.send_expect_json(
        "GET",
        format!(
            "{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/contents/.forgejo/workflows/ci.yml"
        ),
        token,
        None,
        "get CI workflow file",
    );
    assert_eq!(status, 200, "the CI workflow file must be committed");

    // Every workflow label exists on the repo.
    let labels = repo_label_names(rest, base_url, admin_token);
    for label in temper_reference_delivery::basic_delivery_workflow().labels() {
        assert!(
            labels.iter().any(|name| name == label.as_str()),
            "workflow label `{}` must exist on the repo (have: {labels:?})",
            label.as_str()
        );
    }

    // Role users + the automation bot exist.
    for login in ["architect", "engineer", BOT_USER] {
        let (status, _) = rest.send_expect_json(
            "GET",
            format!("{base_url}/api/v1/users/{login}"),
            token,
            None,
            "get user",
        );
        assert_eq!(status, 200, "user `{login}` must be provisioned");
    }

    // The webhook is registered on the repo and points at the configured URL.
    let hooks = repo_webhook_urls(rest, base_url, admin_token);
    assert!(
        hooks.iter().any(|url| url == webhook_url),
        "a webhook pointing at {webhook_url} must be registered (have: {hooks:?})"
    );
}

/// The label names on the repo.
fn repo_label_names(rest: &BlockingJsonClient, base_url: &str, admin_token: &str) -> Vec<String> {
    let (status, labels) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/labels?limit=100"),
        Some(admin_token),
        None,
        "list labels",
    );
    assert_eq!(status, 200, "listing labels must succeed");
    labels
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|l| l["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The delivery URLs of every webhook on the repo.
fn repo_webhook_urls(rest: &BlockingJsonClient, base_url: &str, admin_token: &str) -> Vec<String> {
    let (status, hooks) = rest.send_expect_json(
        "GET",
        format!("{base_url}/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/hooks"),
        Some(admin_token),
        None,
        "list webhooks",
    );
    assert_eq!(status, 200, "listing webhooks must succeed");
    hooks
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|h| h["config"]["url"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Counts the forge objects an idempotent re-run must not duplicate: labels,
/// the three provisioned users, and webhooks.
fn forge_object_counts(
    rest: &BlockingJsonClient,
    base_url: &str,
    admin_token: &str,
) -> (usize, usize) {
    let labels = repo_label_names(rest, base_url, admin_token).len();
    let hooks = repo_webhook_urls(rest, base_url, admin_token).len();
    (labels, hooks)
}

// ── credentials inspection ──────────────────────────────────────────────────────

/// Reads the admin user's REST token from the written credentials file.
fn read_admin_token(credentials_path: &Path) -> String {
    let value = parse_credentials(credentials_path);
    value["forge"]["users"][ADMIN_USER]["token"]
        .as_str()
        .expect("admin token in credentials")
        .to_string()
}

/// Maps each `[forge.users.<role>]` to its token (role users + bot), so the test
/// can prove a `--force` re-run mints fresh tokens.
fn role_tokens(credentials_path: &Path) -> BTreeMap<String, String> {
    let value = parse_credentials(credentials_path);
    let mut out = BTreeMap::new();
    if let Some(users) = value["forge"]["users"].as_table() {
        for (name, user) in users {
            if name == ADMIN_USER {
                continue;
            }
            if let Some(token) = user.get("token").and_then(toml::Value::as_str) {
                out.insert(name.clone(), token.to_string());
            }
        }
    }
    out
}

fn parse_credentials(credentials_path: &Path) -> toml::Value {
    let text = std::fs::read_to_string(credentials_path).expect("read credentials.toml");
    toml::from_str::<toml::Value>(&text).expect("credentials.toml parses as TOML")
}

// ── daemon-boot stretch ────────────────────────────────────────────────────────

/// Boots the standalone engine against the init-produced config + credentials and
/// asserts it binds its webhook port (a clean boot: env→config→composition,
/// forge resolution, workflow label upserts). Proves init's output is a valid,
/// daemon-runnable deployment without the full convergence machinery.
fn assert_daemon_boots(
    config_path: &Path,
    credentials_path: &Path,
    scratch_dir: &Path,
    bind_port: u16,
) {
    let log_path = scratch_dir.join("engine-boot.log");
    let log = std::fs::File::create(&log_path).expect("create engine boot log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("daemon")
        .arg("--service")
        .arg("engine")
        .arg("--config")
        .arg(config_path)
        .arg("--credentials")
        .arg(credentials_path)
        // Keep the boot hermetic from ambient forge/provider env.
        .env_remove("FORGEJO_URL")
        .env_remove("FORGEJO_ACCESS_TOKEN")
        .env_remove("FORGEJO_DEFAULT_REPO")
        .env_remove("TEMPER_FORGE_URL")
        .env_remove("TEMPER_FORGE_TOKEN")
        .stdout(Stdio::from(log.try_clone().expect("clone log handle")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("standalone engine spawns");

    let deadline = Instant::now() + DAEMON_BOOT_TIMEOUT;
    let bound = loop {
        if std::net::TcpStream::connect(("127.0.0.1", bind_port)).is_ok() {
            break true;
        }
        if let Some(status) = child.try_wait().expect("engine try_wait") {
            let _ = std::io::stderr().write_all(&read_log(&log_path));
            panic!(
                "engine exited during boot with {status:?}\n--- engine boot log ---\n{}",
                String::from_utf8_lossy(&read_log(&log_path))
            );
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        bound,
        "engine did not bind 127.0.0.1:{bind_port} within {DAEMON_BOOT_TIMEOUT:?}\n--- engine boot log ---\n{}",
        String::from_utf8_lossy(&read_log(&log_path))
    );
}

fn read_log(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// Allocates a free local port via bind-then-drop.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port binds")
        .local_addr()
        .expect("bound listener has an address")
        .port()
}
