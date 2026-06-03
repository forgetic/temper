//! Gated real-Forgejo multi-repo e2e with webhook wakeups.
//!
//! One fixed fake-agent worker set (one role worker per role plus one mechanical
//! worker) receives two `--repo` values, while a real throwaway Forgejo server
//! and real host-mode `forgejo-runner` provide backend state and CI. Webhooks are
//! registered for both repos and wake the shared worker set; the poll interval is
//! intentionally longer than the convergence budget.

#![cfg(unix)]

#[path = "support/forgejo_multi_repo.rs"]
mod support;

use std::time::{Duration, Instant};
use temper_runner::Scenario;
use temper_testing::forgejo_server::{ForgejoRunner, ForgejoServer};
use temper_testing::runner_config;
use temper_testing::scenarios::{cross_repo_fanout_converges, happy_path};

#[test]
#[ignore = "boots real Forgejo + forgejo-runner and opens local sockets; run with --ignored"]
fn one_fixed_worker_set_processes_two_forgejo_repos_by_webhook_wake() {
    run_webhook_variant(WebhookVariant {
        second_repo: "service-beta",
        scenario: happy_path,
        architect: "default",
        seed: SeedMode::EveryRepo,
    });
}

#[test]
#[ignore = "boots real Forgejo + forgejo-runner and opens local sockets; run with --ignored"]
fn cross_repo_fanout_converges_by_webhook_wake() {
    run_webhook_variant(WebhookVariant {
        second_repo: "service-canary",
        scenario: cross_repo_fanout_converges,
        architect: "closing",
        seed: SeedMode::SourceRepoOnly,
    });
}

struct WebhookVariant {
    second_repo: &'static str,
    scenario: fn() -> Scenario,
    architect: &'static str,
    seed: SeedMode,
}

#[derive(Clone, Copy)]
enum SeedMode {
    EveryRepo,
    SourceRepoOnly,
}

fn run_webhook_variant(variant: WebhookVariant) {
    let server = ForgejoServer::start().expect("forgejo server boots");
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");

    let provisioned = support::block_on_provision(&server);
    let second_name = variant.second_repo.to_string();
    let second_repo = support::futures_block_on(support::provision_extra_repo(
        &server,
        &provisioned,
        &second_name,
    ))
    .expect("second repo provisions");
    let repos = vec![
        support::RepoTarget::from_provisioned(&provisioned),
        support::RepoTarget {
            owner: provisioned.owner.clone(),
            name: second_name,
            id: second_repo,
        },
    ];

    let run_dir = server
        .data_dir()
        .join(format!("multi-repo-webhook-{}", variant.second_repo));
    let log_dir = run_dir.join("logs");
    let wake_dir = run_dir.join("wake");
    std::fs::create_dir_all(&log_dir).expect("log dir is created");
    std::fs::create_dir_all(&wake_dir).expect("wake dir is created");
    let stop_file = run_dir.join("stop");
    let webhook_secret = run_dir.join("webhook-secret");
    let wake_secret = run_dir.join("wake-secret");
    std::fs::write(&webhook_secret, "webhook-secret\n").expect("webhook secret is written");
    std::fs::write(&wake_secret, "wake-secret\n").expect("wake secret is written");

    let trigger_addr = support::free_addr();
    support::start_trigger(
        trigger_addr,
        webhook_secret.clone(),
        wake_secret.clone(),
        wake_dir.clone(),
    );
    support::wait_for_trigger(trigger_addr);
    for repo in &repos {
        support::register_webhook(
            &server,
            &provisioned.admin_token,
            repo,
            trigger_addr,
            &webhook_secret,
        );
    }

    let scenario = (variant.scenario)();
    let mut workers = support::WorkerFleet::spawn_with_behavior(
        &server,
        &provisioned,
        &repos,
        &stop_file,
        &wake_dir,
        &wake_secret,
        &log_dir,
        &runner_config(),
        variant.architect,
        "default",
    );
    workers.wait_for_initial_ticks(Duration::from_secs(30));

    let started = Instant::now();
    match variant.seed {
        SeedMode::EveryRepo => {
            for repo in &repos {
                support::seed(&server, &provisioned, repo, &scenario);
            }
        }
        SeedMode::SourceRepoOnly => support::seed(&server, &provisioned, &repos[0], &scenario),
    }
    let converged = match variant.seed {
        SeedMode::EveryRepo => {
            support::poll_until_all_converged(&server, &provisioned, &repos, &scenario)
        }
        SeedMode::SourceRepoOnly => {
            support::poll_until_converged(&server, &provisioned, &repos[0], &scenario)
        }
    };
    let elapsed = started.elapsed();

    support::touch(&stop_file);
    let exits = workers.wait_all();

    if let Err(error) = converged {
        panic!(
            "multi-repo Forgejo webhook e2e did not converge within {:?}:\n{error}\n\
             elapsed={elapsed:?}, poll_interval={}ms\n\
             repos={}\n\
             trigger URL=http://{trigger_addr}/forgejo/webhook\n\
             worker logs under {}:\n{}\n--- runner running={} log ---\n{}\n--- CI diagnostics ---\n{}",
            support::CONVERGENCE_TIMEOUT,
            support::LONG_POLL_MS,
            repos.iter()
                .map(support::RepoTarget::display)
                .collect::<Vec<_>>()
                .join(","),
            log_dir.display(),
            workers.logs(),
            runner.is_running(),
            runner.log_tail(),
            support::ci_diagnostics(&server, &provisioned, &repos),
        );
    }

    assert!(
        elapsed < Duration::from_millis(support::LONG_POLL_MS),
        "converged in {elapsed:?}, which is not before the {}ms poll backstop",
        support::LONG_POLL_MS
    );
    assert!(
        workers.logs().contains("consumed authenticated wake"),
        "no worker consumed an authenticated wake; logs:\n{}",
        workers.logs()
    );
    for (label, status) in &exits {
        assert!(status.success(), "worker '{label}' exited with {status:?}");
    }
}

#[test]
#[cfg(not(unix))]
#[ignore]
fn one_fixed_worker_set_processes_two_forgejo_repos_by_webhook_wake() {
    eprintln!("skipping Forgejo multi-repo webhook e2e: Unix wake sockets are required");
}

#[test]
#[cfg(not(unix))]
#[ignore]
fn cross_repo_fanout_converges_by_webhook_wake() {
    eprintln!("skipping Forgejo cross-repo webhook e2e: Unix wake sockets are required");
}
