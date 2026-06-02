//! Phase 3b e2e: read real CI status through the password/web-UI path.
//!
//! `#[ignore]`d **and** gated behind `TEMPER_FORGEJO_E2E=1`, so the default
//! `cargo test` never downloads a binary, opens a socket, or spawns a server —
//! exactly like the Phase 1/1b/2 e2e tests. Run it with:
//!
//! ```sh
//! TEMPER_FORGEJO_E2E=1 \
//!   cargo test -p temper-testing --test forgejo_ci_web_ui -- --ignored
//! ```
//!
//! It boots a [`ForgejoServer`] + [`ForgejoRunner`] (host mode, no containers),
//! provisions the repo (whose `.forgejo/workflows/ci.yml` fails on a head with no
//! `ci-ok` sentinel — the committed `main` head has none), and then drives a real
//! [`ForgejoForge`] configured with web-UI credentials. Because Forgejo 7.0.12
//! does not serve `actions/runs` over REST, `list_ci_jobs` transparently falls
//! back to the password/web-UI read path (ADR 0019), logging in and reading the
//! run's live-view JSON. The test polls until a `CiJob` with a `Failure`
//! conclusion comes back — proving the read path works end to end against a real
//! runner-produced verdict.

use std::time::{Duration, Instant};
use temper_forge::{CiJobConclusion, CiJobQuery, CiJobStatus};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_server::{provision, ForgejoRunner, ForgejoServer};
use temper_testing::runner_config;

/// Returns whether the env opt-in is present; prints a skip note when not.
fn enabled() -> bool {
    if std::env::var("TEMPER_FORGEJO_E2E").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!(
        "skipping Forgejo CI web-UI e2e test: set TEMPER_FORGEJO_E2E=1 to enable \
         (downloads pinned Forgejo + forgejo-runner binaries and spawns a host-mode runner)"
    );
    false
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots a real Forgejo + host-mode runner; run with TEMPER_FORGEJO_E2E=1 -- --ignored"]
async fn list_ci_jobs_reads_failure_through_web_ui() {
    if !enabled() {
        return;
    }

    // `ForgejoServer::start` uses a *blocking* reqwest client for its readiness
    // poll; boot it on a blocking thread so that nested runtime lives and dies
    // off-reactor (matching the Phase 2 provisioning test).
    let server = tokio::task::spawn_blocking(ForgejoServer::start)
        .await
        .expect("server boot task joins")
        .expect("forgejo server boots");
    let base = server.base_url().to_string();

    // Register the host-mode runner before provisioning so it is ready to claim
    // the run the workflow commit triggers.
    let mut runner = ForgejoRunner::register(&server).expect("runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let provisioned = provision(&server).await.expect("provisioning succeeds");

    // Build a backend as one workflow role: the role's token for REST plus that
    // role's user/password for the web-UI CI read fallback (ADR 0019).
    let binding = runner_config()
        .role_bindings
        .into_iter()
        .next()
        .expect("at least one role binding");
    let identity = provisioned.role(&binding.role).expect("role provisioned");
    let config = ForgejoConfig::new(&base, &identity.token)
        .with_web_ui_credentials(&identity.user, &identity.password);
    let forge = ForgejoForge::new(config);

    // The workflow commit on `main` triggers a push run that fails (`test -f
    // ci-ok` with no sentinel). Poll until the web-UI read path returns a failed
    // job. A real host CI job takes seconds; allow generous slack for cold start.
    let deadline = Instant::now() + Duration::from_secs(180);
    // Most recent observation, surfaced in the timeout panic for diagnosis.
    let mut last = String::from("(no jobs yet)");
    let _ = &last;
    let failing = loop {
        match forge
            .list_ci_jobs(&provisioned.repository, CiJobQuery::default())
            .await
        {
            Ok(jobs) => {
                last = format!(
                    "{:?}",
                    jobs.iter()
                        .map(|j| (j.name.clone(), j.status, j.conclusion))
                        .collect::<Vec<_>>()
                );
                if let Some(job) = jobs.iter().find(|job| {
                    job.status == CiJobStatus::Completed
                        && job.conclusion == Some(CiJobConclusion::Failure)
                }) {
                    break Some(job.clone());
                }
            }
            // Transient read errors during cold start are tolerated until the
            // deadline; a persistent failure surfaces below as a timeout.
            Err(error) => last = format!("error: {error}"),
        }
        if Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    };

    let job = failing.unwrap_or_else(|| {
        panic!(
            "expected a Failure CI job read through the web UI; last observation: {last}; \
             runner running={}, log: {}",
            runner.is_running(),
            runner.log_tail()
        )
    });
    assert_eq!(job.status, CiJobStatus::Completed);
    assert_eq!(job.conclusion, Some(CiJobConclusion::Failure));

    // Tear down explicitly so any panic in drop surfaces here, not at unwind.
    drop(runner);
    drop(server);
}
