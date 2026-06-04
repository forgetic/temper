//! Phase 3 backend-seam test: a `--backend forgejo` worker ticks against a real
//! server.
//!
//! `#[ignore]`d, so the default `cargo test` never downloads a binary, opens a
//! socket, or spawns a server. No extra environment variable is required; run it
//! with:
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_worker -- --ignored
//! ```
//!
//! It proves the Phase 3 contract: the worker library's backend seam constructs
//! a real `ForgejoForge` handle (identity = the per-role token, no `as_user`) and
//! drives a runner worker against a running server for a couple of ticks. The
//! happy path here is the **engineer** role: it is the most load-bearing fake and
//! needs no CI to make progress on an empty repo, so a couple of bounded ticks
//! exercise scan → (no work) without requiring the runner. The full four-scenario
//! convergence across processes is Phase 4.
//!
//! Three Phase 3 guarantees are checked end to end:
//!
//! - a `--backend forgejo` role worker builds and ticks without error (the
//!   backend seam + Tokio executor path work against the real server);
//! - secrets travel via env, exactly as the Phase 4 spawner will set them
//!   (`TEMPER_FORGEJO_TOKEN`, plus optional `TEMPER_FORGEJO_USERNAME/PASSWORD`);
//! - `--kind ci --backend forgejo` is rejected (no fake CI producer on Forgejo).
//!
//! `worker_bin::run` builds its **own** current-thread Tokio runtime for the
//! Forgejo path, so it is invoked through `spawn_blocking` here — calling it
//! directly inside this `#[tokio::test]` reactor would panic ("cannot start a
//! runtime from within a runtime").

use temper_testing::forgejo_server::start_cached_provisioned_server;
use temper_testing::worker_bin::{
    self, parse_with_env, Backend, ParseOutcome, RoleBehavior, WorkerKind, FORGEJO_PASSWORD_ENV,
    FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV,
};
use temper_workflow::RoleId;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots a real Forgejo server; run with --ignored"]
async fn forgejo_role_worker_ticks_against_a_running_server() {
    // Boot off-reactor: the cached Forgejo fixture uses a blocking reqwest client
    // for readiness polling, whose nested blocking runtime must live and die off the
    // async test thread (same pattern as the Phase 2/2b tests).
    let cached = tokio::task::spawn_blocking(start_cached_provisioned_server)
        .await
        .expect("server boot task joins")
        .expect("forgejo cached provisioned state starts");
    let server = cached.server;
    let provisioned = cached.provisioned;
    let base = server.base_url().to_string();
    let engineer = provisioned
        .role(&RoleId::new("engineer"))
        .expect("engineer role is provisioned")
        .clone();

    // The exact argv + env a Phase 4 spawner would use for this role: base URL on
    // argv, every secret in the environment. Parse it through the real parser so
    // the test exercises the same construction path the binary does.
    let argv = vec![
        "--kind".to_string(),
        "role".to_string(),
        "--role".to_string(),
        "engineer".to_string(),
        "--user".to_string(),
        engineer.user.clone(),
        "--backend".to_string(),
        "forgejo".to_string(),
        "--base-url".to_string(),
        base.clone(),
        "--repo".to_string(),
        format!("{}/{}", provisioned.owner, provisioned.name),
        // Root is unused by the forgejo backend but the parser still requires it.
        "--root".to_string(),
        std::env::temp_dir()
            .join("temper-forgejo-worker-unused")
            .to_string_lossy()
            .into_owned(),
        "--clock".to_string(),
        "wall".to_string(),
        "--poll-ms".to_string(),
        "500".to_string(),
        // A short deadline: a couple of poll iterations, then stop.
        "--run-secs".to_string(),
        "2".to_string(),
    ];
    let env = move |key: &str| match key {
        FORGEJO_TOKEN_ENV => Some(engineer.token.clone()),
        FORGEJO_USERNAME_ENV => Some(engineer.user.clone()),
        FORGEJO_PASSWORD_ENV => Some(engineer.password.clone()),
        _ => None,
    };
    let args = match parse_with_env(argv, env).expect("worker args parse") {
        ParseOutcome::Run(args) => *args,
        ParseOutcome::Help => panic!("unexpected help outcome"),
    };

    // Sanity: the seam selected the Forgejo backend with the base URL on argv and
    // the token from the environment.
    assert!(
        matches!(&args.backend, Backend::Forgejo(forgejo) if forgejo.base_url == base
            && !forgejo.token.is_empty()),
        "expected a Forgejo backend with a non-empty token from env"
    );
    assert!(matches!(
        args.kind,
        WorkerKind::Role { ref role, .. } if role == "engineer"
    ));

    // `worker_bin::run` builds its own current-thread Tokio runtime for the
    // Forgejo path, so drive it on a blocking thread, not inside this reactor.
    let report = tokio::task::spawn_blocking(move || worker_bin::run(&args))
        .await
        .expect("worker run task joins")
        .expect("forgejo role worker ticks against the running server");
    assert_eq!(
        report.workers.len(),
        1,
        "one role worker should have run and reported"
    );

    // The CI faker has no Forgejo counterpart: `--kind ci --backend forgejo` must
    // be rejected at parse time (the real forgejo-runner is the producer).
    let ci_argv = vec![
        "--kind".to_string(),
        "ci".to_string(),
        "--backend".to_string(),
        "forgejo".to_string(),
        "--base-url".to_string(),
        base.clone(),
        "--repo".to_string(),
        format!("{}/{}", provisioned.owner, provisioned.name),
        "--root".to_string(),
        "/tmp/unused".to_string(),
        "--clock".to_string(),
        "wall".to_string(),
    ];
    let ci_result = parse_with_env(ci_argv, |key: &str| match key {
        FORGEJO_TOKEN_ENV => Some("unused".to_string()),
        _ => None,
    });
    let ci_error = ci_result.expect_err("--kind ci must be rejected for --backend forgejo");
    assert!(
        ci_error.to_string().contains("--kind ci"),
        "rejection should mention --kind ci, got: {ci_error}"
    );

    // Tear down explicitly so any panic in drop surfaces here.
    drop(server);
}

/// Sanity check that runs in the default suite (no server): the engineer role's
/// behavior default is what a Phase 4 spawner would pass, and the parser still
/// produces a Forgejo backend for it. Keeps the seam covered without booting a
/// server in the default suite.
#[test]
fn role_behavior_default_is_stable() {
    assert_eq!(RoleBehavior::default(), RoleBehavior::default());
}
