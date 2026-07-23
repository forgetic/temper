// SPDX-License-Identifier: MPL-2.0

//! The §7 startup/health banner for the standalone (`mode=standalone`) daemon.
//!
//! These are the boot-block lines from the logging design's reference output
//! (`docs/explanation/logging-and-observability.md`, §7): the `engine: …`
//! readiness lines, the `worker: capacity:` line for compiled workflow role
//! limits, and the `trigger: webhook listener up …` line. They are emitted
//! through `temper-log`'s service-status helpers
//! ([`temper_log::emit::emit_engine_status`] and friends), which set the
//! `service=` machine field; the bodies built here carry **no** service prefix
//! (the prefix is the human projection's concern — see
//! [`temper_log::Service::human_prefix`]).
//!
//! Every line body is rendered by a pure function over plain values pulled from
//! the daemon's *resolved* config (repos, roles, workflow name, queue count, bind
//! address, backstop cadences, the forge URL + bot login), never the §7 sample
//! constants. The pure split keeps the exact ASCII contract unit-testable while
//! the `run_async` orchestration only decides *when* each line fires.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

/// Renders the `temper <version> starting | mode=standalone pid=<pid>` line.
pub(crate) fn starting(version: &str, pid: u32) -> String {
    format!("temper {version} starting | mode=standalone pid={pid}")
}

/// Renders the `config loaded from <path>` line.
///
/// When the deployment was assembled without an on-disk config (env/defaults
/// only), there is no path to name, so the source is reported as `defaults`.
pub(crate) fn config_loaded(path: Option<&Path>) -> String {
    match path {
        Some(path) => format!("config loaded from {}", path.display()),
        None => "config loaded from defaults".to_string(),
    }
}

/// Renders the `forge: forgejo @ <url> (reachable, auth ok as <login>)` line,
/// emitted after a successful `current_user` auth/connectivity probe.
pub(crate) fn forge_reachable(url: &str, bot_login: &str) -> String {
    format!("forge: forgejo @ {url} (reachable, auth ok as {bot_login})")
}

/// Renders the `workflow: <name> | roles=<...> | queues=<N>` line.
pub(crate) fn workflow(name: &str, roles: &[String], queue_count: usize) -> String {
    format!(
        "workflow: {name} | roles={} | queues={queue_count}",
        roles.join(",")
    )
}

/// Renders the `watching <N> repos: <repo, repo>` line.
pub(crate) fn watching(repos: &[String]) -> String {
    format!("watching {} repos: {}", repos.len(), repos.join(", "))
}

/// Renders the per-repo `repo <owner/repo>: labels verified (<label,...>)` line.
pub(crate) fn repo_labels(repo: &str, labels: &[String]) -> String {
    format!("repo {repo}: labels verified ({})", labels.join(","))
}

/// Renders the `planes up: engine + worker + agent on one loop` line.
pub(crate) fn planes_up() -> String {
    "planes up: engine + worker + agent on one loop".to_string()
}

/// Renders the `webhook listener up on <bind>/forgejo/webhook (issue, PR, CI
/// events)` line (a `trigger:` status line).
pub(crate) fn webhook_listener(bind: &str) -> String {
    format!("webhook listener up on {bind}/forgejo/webhook (issue, PR, CI events)")
}

/// Renders the `poll backstop every <Ns> (<roles> feeds across <N> repos)` line.
pub(crate) fn poll_backstop(cadence: Duration, roles: &[String], repo_count: usize) -> String {
    format!(
        "poll backstop every {} ({} feeds across {repo_count} repos)",
        humanize_secs(cadence),
        roles.join(", ")
    )
}

/// Renders the dedicated CI-status poll cadence and missing-current-head grace.
/// The disabled state explains that a configured grace does not activate
/// missing-CI detection or parking by itself.
pub(crate) fn ci_poll_backstop(
    cadence: Option<Duration>,
    missing_grace: Duration,
    repo_count: usize,
) -> String {
    match cadence {
        Some(cadence) => format!(
            "CI-status poll backstop every {} (CI-gated pull requests across {repo_count} repos; missing-current-head grace {})",
            humanize_secs(cadence),
            humanize_secs(missing_grace)
        ),
        None => format!(
            "CI-status poll backstop disabled (missing-current-head detection and parking inactive; configured grace {})",
            humanize_secs(missing_grace)
        ),
    }
}

/// Renders the `mechanical backstop every <Ns> (raw_intake, landing across <N>
/// repos)` line.
pub(crate) fn mechanical_backstop(cadence: Duration, repo_count: usize) -> String {
    format!(
        "mechanical backstop every {} (raw_intake, landing across {repo_count} repos)",
        humanize_secs(cadence)
    )
}

/// Renders each compiled workflow role's global concurrency limit in declaration
/// order. `None` is explicit rather than replaced with worker-advertised local
/// capacity, because an unlimited role is still bounded by available workers.
pub(crate) fn capacity(roles: &[(String, Option<u32>)]) -> String {
    let mut line = "capacity: ".to_string();
    for (role, concurrency) in roles {
        match concurrency {
            Some(limit) => {
                let _ = write!(line, "{role}={limit} ");
            }
            None => {
                let _ = write!(line, "{role}=unlimited ");
            }
        }
    }
    line.push_str("(per-role, shared across all repos)");
    line
}

/// Renders the `ready -- watching <repos>, idle` line, the readiness signal
/// operators wait for.
///
/// Note: §7 also defines a steady-state closing `idle -- watching <repos>` line,
/// emitted when in-flight work *drains* back to idle. That transition is detected
/// by the engine/runner's work loop (WI-2's surface), not the standalone
/// assembly wired here, so this banner does not own an emit point for it.
pub(crate) fn ready(repos: &[String]) -> String {
    format!("ready -- watching {}, idle", repos.join(", "))
}

/// Formats a cadence as the §7 `<N>s` token. §7 renders both backstop cadences
/// in whole seconds (`every 60s`, `every 30s`), so the banner keeps that single,
/// unambiguous unit rather than switching to minutes at 60.
fn humanize_secs(cadence: Duration) -> String {
    format!("{}s", cadence.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos() -> Vec<String> {
        vec!["acme/widgets".to_string(), "acme/api".to_string()]
    }

    fn role_capacities() -> Vec<(String, Option<u32>)> {
        vec![
            ("architect".to_string(), Some(2)),
            ("engineer".to_string(), None),
            ("mechanical".to_string(), Some(1)),
        ]
    }

    #[test]
    fn starting_line_matches_spec_shape() {
        assert_eq!(
            starting("0.9.0", 4821),
            "temper 0.9.0 starting | mode=standalone pid=4821"
        );
    }

    #[test]
    fn config_loaded_names_path_or_defaults() {
        assert_eq!(
            config_loaded(Some(Path::new("/etc/temper/temper.toml"))),
            "config loaded from /etc/temper/temper.toml"
        );
        assert_eq!(config_loaded(None), "config loaded from defaults");
    }

    #[test]
    fn forge_line_matches_spec() {
        assert_eq!(
            forge_reachable("https://git.example.com", "temper-bot"),
            "forge: forgejo @ https://git.example.com (reachable, auth ok as temper-bot)"
        );
    }

    #[test]
    fn workflow_line_joins_roles_and_counts_queues() {
        assert_eq!(
            workflow(
                "basic-delivery",
                &[
                    "architect".to_string(),
                    "engineer".to_string(),
                    "mechanical".to_string(),
                ],
                5,
            ),
            "workflow: basic-delivery | roles=architect,engineer,mechanical | queues=5"
        );
    }

    #[test]
    fn watching_line_counts_and_lists_repos() {
        assert_eq!(
            watching(&repos()),
            "watching 2 repos: acme/widgets, acme/api"
        );
    }

    #[test]
    fn repo_labels_line_matches_spec() {
        assert_eq!(
            repo_labels(
                "acme/widgets",
                &[
                    "untriaged".to_string(),
                    "code".to_string(),
                    "ready".to_string()
                ]
            ),
            "repo acme/widgets: labels verified (untriaged,code,ready)"
        );
    }

    #[test]
    fn planes_and_webhook_lines_match_spec() {
        assert_eq!(
            planes_up(),
            "planes up: engine + worker + agent on one loop"
        );
        assert_eq!(
            webhook_listener(":8080"),
            "webhook listener up on :8080/forgejo/webhook (issue, PR, CI events)"
        );
    }

    #[test]
    fn poll_backstop_humanizes_cadence_and_names_roles() {
        assert_eq!(
            poll_backstop(
                Duration::from_secs(60),
                &["architect".to_string(), "engineer".to_string()],
                2
            ),
            "poll backstop every 60s (architect, engineer feeds across 2 repos)"
        );
    }

    #[test]
    fn ci_poll_backstop_reports_effective_cadence_grace_and_disabled_state() {
        assert_eq!(
            ci_poll_backstop(Some(Duration::from_secs(60)), Duration::from_secs(300), 2),
            "CI-status poll backstop every 60s (CI-gated pull requests across 2 repos; missing-current-head grace 300s)"
        );
        assert_eq!(
            ci_poll_backstop(None, Duration::from_secs(45), 2),
            "CI-status poll backstop disabled (missing-current-head detection and parking inactive; configured grace 45s)"
        );
    }

    #[test]
    fn mechanical_backstop_line_matches_spec() {
        assert_eq!(
            mechanical_backstop(Duration::from_secs(30), 2),
            "mechanical backstop every 30s (raw_intake, landing across 2 repos)"
        );
    }

    #[test]
    fn capacity_line_states_compiled_limits_and_unlimited_roles() {
        assert_eq!(
            capacity(&role_capacities()),
            "capacity: architect=2 engineer=unlimited mechanical=1 (per-role, shared across all repos)"
        );
    }

    #[test]
    fn worker_capacity_does_not_control_workflow_capacity_line() {
        let worker_max_concurrent_jobs = 99_u32;
        let line = capacity(&role_capacities());

        assert!(!line.contains(&format!("={worker_max_concurrent_jobs}")));
        assert!(line.contains("engineer=unlimited"));
    }

    #[test]
    fn ready_line_matches_spec() {
        assert_eq!(
            ready(&repos()),
            "ready -- watching acme/widgets, acme/api, idle"
        );
    }

    #[test]
    fn humanize_renders_whole_seconds() {
        assert_eq!(humanize_secs(Duration::from_secs(30)), "30s");
        assert_eq!(humanize_secs(Duration::from_secs(60)), "60s");
        assert_eq!(humanize_secs(Duration::from_secs(120)), "120s");
        assert_eq!(humanize_secs(Duration::from_secs(0)), "0s");
    }
}
