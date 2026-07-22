//! `temper init` live e2e: drive the library entry point `run_init` against a
//! real Forgejo (the shared bench fixture) and prove the produced config +
//! credentials stand up a valid, daemon-runnable deployment.
//!
//! This is the capstone of plan #176. Where the unit test in
//! `temper-cli-init` stubs the live-forge step, this test swaps in the **real**
//! [`ForgejoProvisioner`] and runs the whole flow end to end:
//!
//!   collect (scripted answers) → write local files → apply/provision the live forge
//!   → update credentials → summarize
//!
//! and then asserts BOTH sides of the seam:
//!
//! 1. **Local artifacts** — `config.toml` parses and `temper_config::load`
//!    resolves with the expected forge URL, repo, roles, workspace, and webhook
//!    secret; `lint` reports no errors; `workflow.yaml` parses as YAML and
//!    matches the embedded basic-delivery workflow; `credentials.toml` is `0600`
//!    and holds the minted role/bot tokens, the admin identity, and the DeepSeek
//!    key.
//! 2. **Forge state** — the org/owner, the repo (Actions enabled, with no
//!    project/CI files committed), every basic-delivery label, the role users +
//!    `bot`, and the webhook pointing at the configured address all exist on the
//!    live forge.
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
//! Run with `cargo test --test init_forgejo_e2e -- --ignored`, or via the
//! default live capstone lane (`cargo dev-test-full` /
//! `cargo dev-test-e2e-capstones`).

#![cfg(unix)]

#[path = "init_forgejo_e2e/credentials.rs"]
mod credentials;
#[path = "init_forgejo_e2e/daemon_boot.rs"]
mod daemon_boot;
#[path = "support/e2e_lock.rs"]
mod e2e_lock;
#[path = "init_forgejo_e2e/forge_state.rs"]
mod forge_state;
#[path = "init_forgejo_e2e/local_artifacts.rs"]
mod local_artifacts;

use std::time::{Duration, Instant};

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{ForgejoProvisioner, InitOptions, run_init};
use temper_engine_io::http::BlockingJsonClient;
use temper_testing::forgejo_server::start_cached_bare_admin_server;

use credentials::{read_admin_token, role_tokens};
use daemon_boot::{assert_daemon_boots, free_port};
use forge_state::{assert_forge_state, forge_object_counts};
use local_artifacts::assert_local_artifacts;

/// A non-reserved Forgejo admin login (`admin` itself is reserved). The cached
/// bare-admin fixture creates it while priming the Forgejo state, then the test
/// hands it to init as the scripted Q4 answer so the real provisioner mints an
/// admin REST token from it.
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

const DAEMON_BOOT_TIMEOUT: Duration = Duration::from_secs(120);

#[test]
#[ignore = "boots a real Forgejo fixture and provisions it via `temper init --apply`; run with --ignored"]
fn init_forgejo_drives_a_working_setup() {
    let _e2e_lock = e2e_lock::acquire();
    let started = Instant::now();

    // --- A bare Forgejo with a site admin, but NOTHING else provisioned. ---
    // Reuse the cached bare-admin fixture instead of paying the cold Forgejo
    // first-start + CLI admin-create cost in this capstone. We deliberately do
    // not use the cached *provisioned* world: the whole point is to let
    // `run_init`'s real ForgejoProvisioner create the org, repo, users, labels,
    // webhook, and CI enablement itself, without seeding project files.
    let cached = start_cached_bare_admin_server(ADMIN_USER, ADMIN_PASSWORD, ADMIN_EMAIL)
        .expect("cached bare-admin Forgejo fixture starts");
    assert_eq!(cached.admin.username, ADMIN_USER);
    assert_eq!(cached.admin.email, ADMIN_EMAIL);
    let server = cached.server;
    let base_url = server.base_url().to_string();
    eprintln!(
        "init_forgejo_e2e: Forgejo up at {base_url} cache_hit={} (startup {:?})",
        cached.cache_hit,
        started.elapsed()
    );

    // The webhook address doubles as the engine bind, so use a real free port on
    // 127.0.0.1 (a hostname like `localhost` does not parse as a SocketAddr).
    let bind_port = free_port();
    let webhook_addr = format!("http://127.0.0.1:{bind_port}");

    // Per-test temp config dir: config.toml, credentials.toml, workflow.yaml, and
    // the webhook secret all land here; the real ~/.config/temper is never read
    // or written.
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");
    let credentials_path = config_dir.path().join("credentials.toml");
    let workflow_path = config_dir.path().join("workflow.yaml");
    let webhook_secret_path = config_dir.path().join("webhook-secret");
    let workspace_dir = tempfile::tempdir().expect("workspace tempdir");

    let opts = InitOptions {
        options: LoadOptions {
            config: Some(config_path.clone()),
            credentials: Some(credentials_path.clone()),
        },
        force: false,
        existing_repo: false,
        apply: true,
        yes: true,
        workspace: Some(workspace_dir.path().to_path_buf()),
        ..Default::default()
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
    let webhook_url = format!("{webhook_addr}/forgejo/webhook");
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
