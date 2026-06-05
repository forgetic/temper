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
use temper_testing::forgejo_runtime::{RunWorkspace, TriggerServer};
use temper_testing::forgejo_server::{start_cached_provisioned_repositories, ForgejoRunner};
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
    let config = runner_config();
    let second_name = variant.second_repo.to_string();
    let cached = start_cached_provisioned_repositories(&[
        config.repository.name.clone(),
        second_name.clone(),
    ])
    .expect("forgejo cached provisioned multi-repo state starts");
    let server = cached.server;
    let provisioned = cached
        .state
        .provisioned(&config.repository.name)
        .expect("primary repo is in cached state");
    let second_repo = cached
        .state
        .repositories
        .get(&second_name)
        .expect("second repo is in cached state")
        .clone();
    let mut runner = ForgejoRunner::register(&server).expect("forgejo runner registers");
    assert!(runner.is_running(), "runner daemon exited immediately");
    let repos = vec![
        support::RepoTarget::from_provisioned(&provisioned),
        support::RepoTarget {
            owner: provisioned.owner.clone(),
            name: second_name,
            id: second_repo,
        },
    ];

    let workspace = RunWorkspace::new(format!(
        "temper-forgejo-multi-repo-webhook-{}",
        variant.second_repo
    ));
    let log_dir = workspace.dir("logs");
    let wake_dir = workspace.dir("wake");
    let worker_root_dir = workspace.dir("worker-roots");
    let stop_file = workspace.join("stop");
    let webhook_secret = workspace.write_file("secrets/webhook", "webhook-secret\n");
    let wake_secret = workspace.write_file("secrets/wake", "wake-secret\n");

    let trigger = TriggerServer::start(
        webhook_secret.clone(),
        Some(wake_secret.clone()),
        wake_dir.clone(),
    );
    let trigger_addr = trigger.addr();
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
        &worker_root_dir,
        &runner_config(),
        variant.architect,
        "default",
    );
    workers.wait_for_initial_ticks(Duration::from_secs(30));
    std::thread::sleep(Duration::from_millis(1_500));
    let pre_seed_log_offsets = workers.log_offsets();

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
    if matches!(variant.seed, SeedMode::SourceRepoOnly) {
        let expected_repo = repos[0].display();
        let wake_lines = workers.wake_scan_lines_since(&pre_seed_log_offsets);
        assert!(
            !wake_lines.is_empty(),
            "no completed wake scan lines found; logs:\n{}",
            workers.logs()
        );
        let role_wake_lines = wake_lines
            .into_iter()
            .filter(|line| line.starts_with("role:"))
            .collect::<Vec<_>>();
        assert!(
            role_wake_lines.iter().any(|line| {
                line.contains("scanned_repositories=1")
                    && line.contains(&format!("scanned_repository_paths={expected_repo}"))
            }),
            "no narrowed source-repo role wake found for {expected_repo}; role wake lines:\n{}\nlogs:\n{}",
            role_wake_lines.join("\n"),
            workers.logs()
        );
    }
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
