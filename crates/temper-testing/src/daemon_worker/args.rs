use std::path::PathBuf;

use temper_protocol_worker::Capability;

/// Default long-poll wait. Short enough that the stop-file is honored promptly.
const DEFAULT_POLL_WAIT_MS: u64 = 2_000;

/// Command-line usage for the deterministic daemon test worker.
pub const USAGE: &str = concat!(
    "temper-testing-daemon-worker --daemon-url <http://host:port> ",
    "--worker-id <id> --capability <owner/name:role> [--capability ...] ",
    "--git-base-url <url> --workspace-root <dir> --stop-file <path> ",
    "[--ci-sentinel present|deferred] [--poll-wait-ms <n>]\n",
    "  Git identity comes from TEMPER_E2E_GIT_USER (required) and ",
    "TEMPER_E2E_GIT_TOKEN (optional; omit for file:// remotes)"
);

/// Whether the worker's commit message carries the CI pass marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CiSentinel {
    /// Subject line includes [`crate::daemon_worker::CI_PASS_MARKER`]; CI passes
    /// on the pushed head.
    Present,
    /// Marker omitted; CI fails until a marker-bearing commit lands later.
    Deferred,
}

/// Fully parsed daemon test worker configuration.
#[derive(Clone, Debug)]
pub struct DaemonWorkerConfig {
    pub daemon_url: String,
    pub worker_id: String,
    pub capabilities: Vec<Capability>,
    pub git_base_url: String,
    pub workspace_root: PathBuf,
    pub stop_file: PathBuf,
    pub ci_sentinel: CiSentinel,
    pub poll_wait_ms: u64,
}

/// Result of parsing command-line arguments.
#[derive(Clone, Debug)]
pub enum ParseOutcome {
    Help,
    Run(DaemonWorkerConfig),
}

/// Parses the daemon test worker command-line surface.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<ParseOutcome, String> {
    let mut daemon_url = None;
    let mut worker_id = None;
    let mut capabilities = Vec::new();
    let mut git_base_url = None;
    let mut workspace_root = None;
    let mut stop_file = None;
    let mut ci_sentinel = CiSentinel::Present;
    let mut poll_wait_ms = DEFAULT_POLL_WAIT_MS;

    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--daemon-url" => daemon_url = Some(value_for(&flag, &mut iter)?),
            "--worker-id" => worker_id = Some(value_for(&flag, &mut iter)?),
            "--capability" => capabilities.push(parse_capability(&value_for(&flag, &mut iter)?)?),
            "--git-base-url" => git_base_url = Some(value_for(&flag, &mut iter)?),
            "--workspace-root" => {
                workspace_root = Some(PathBuf::from(value_for(&flag, &mut iter)?))
            }
            "--stop-file" => stop_file = Some(PathBuf::from(value_for(&flag, &mut iter)?)),
            "--ci-sentinel" => {
                ci_sentinel = match value_for(&flag, &mut iter)?.as_str() {
                    "present" => CiSentinel::Present,
                    "deferred" => CiSentinel::Deferred,
                    other => {
                        return Err(format!(
                            "--ci-sentinel must be 'present' or 'deferred', got '{other}'"
                        ));
                    }
                }
            }
            "--poll-wait-ms" => {
                let raw = value_for(&flag, &mut iter)?;
                poll_wait_ms = raw
                    .parse::<u64>()
                    .map_err(|error| format!("--poll-wait-ms must be an integer: {error}"))?;
                if poll_wait_ms == 0 {
                    return Err("--poll-wait-ms must be positive".to_string());
                }
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }

    if capabilities.is_empty() {
        return Err("missing required --capability".to_string());
    }
    Ok(ParseOutcome::Run(DaemonWorkerConfig {
        daemon_url: daemon_url.ok_or("missing required --daemon-url")?,
        worker_id: worker_id.ok_or("missing required --worker-id")?,
        capabilities,
        git_base_url: git_base_url.ok_or("missing required --git-base-url")?,
        workspace_root: workspace_root.ok_or("missing required --workspace-root")?,
        stop_file: stop_file.ok_or("missing required --stop-file")?,
        ci_sentinel,
        poll_wait_ms,
    }))
}

fn value_for(flag: &str, iter: &mut impl Iterator<Item = String>) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_capability(raw: &str) -> Result<Capability, String> {
    let (repo, role) = raw
        .split_once(':')
        .ok_or_else(|| format!("--capability must be owner/name:role, got '{raw}'"))?;
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| format!("--capability repo must be owner/name, got '{raw}'"))?;
    if owner.is_empty() || name.is_empty() || name.contains('/') || role.is_empty() {
        return Err(format!("--capability must be owner/name:role, got '{raw}'"));
    }
    Ok(Capability {
        role: role.to_string(),
        repo: repo.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    fn required() -> Vec<String> {
        strings(&[
            "--daemon-url",
            "http://127.0.0.1:8080",
            "--worker-id",
            "w1",
            "--capability",
            "acme/service:engineer",
            "--git-base-url",
            "http://127.0.0.1:3000",
            "--workspace-root",
            "/tmp/ws",
            "--stop-file",
            "/tmp/stop",
        ])
    }

    fn run_config(args: Vec<String>) -> DaemonWorkerConfig {
        match parse_args(args).expect("arguments parse") {
            ParseOutcome::Run(config) => config,
            ParseOutcome::Help => panic!("expected run config"),
        }
    }

    #[test]
    fn parses_required_flags_with_defaults() {
        let config = run_config(required());

        assert_eq!(config.daemon_url, "http://127.0.0.1:8080");
        assert_eq!(config.worker_id, "w1");
        assert_eq!(
            config.capabilities,
            vec![Capability {
                role: "engineer".to_string(),
                repo: "acme/service".to_string(),
            }]
        );
        assert_eq!(config.git_base_url, "http://127.0.0.1:3000");
        assert_eq!(config.workspace_root, PathBuf::from("/tmp/ws"));
        assert_eq!(config.stop_file, PathBuf::from("/tmp/stop"));
        assert_eq!(config.ci_sentinel, CiSentinel::Present);
        assert_eq!(config.poll_wait_ms, DEFAULT_POLL_WAIT_MS);
    }

    #[test]
    fn parses_deferred_sentinel_and_poll_wait() {
        let mut args = required();
        args.extend(strings(&[
            "--ci-sentinel",
            "deferred",
            "--poll-wait-ms",
            "500",
        ]));

        let config = run_config(args);

        assert_eq!(config.ci_sentinel, CiSentinel::Deferred);
        assert_eq!(config.poll_wait_ms, 500);
    }

    #[test]
    fn rejects_malformed_capabilities() {
        for raw in ["acme/service", "acme:engineer", "acme/:engineer", "a/b:"] {
            let mut args = required();
            args.extend(strings(&["--capability", raw]));
            let error = parse_args(args).unwrap_err();
            assert!(error.contains("--capability"), "error for {raw:?}: {error}");
        }
    }

    #[test]
    fn missing_required_flag_is_rejected() {
        let args = strings(&["--daemon-url", "http://127.0.0.1:8080"]);
        let error = parse_args(args).unwrap_err();
        assert!(error.contains("--capability"));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let mut args = required();
        args.push("--mystery".to_string());
        let error = parse_args(args).unwrap_err();
        assert!(error.contains("--mystery"));
    }

    #[test]
    fn help_flag_short_circuits() {
        assert!(matches!(
            parse_args(strings(&["--help"])).expect("help parses"),
            ParseOutcome::Help
        ));
    }
}
