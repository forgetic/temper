use std::time::Duration;

use super::parse_test_support::{parse_err, parse_ok};
use super::*;

#[test]
fn parses_defaults_and_trims_required_values() {
    let config = parse_ok(&[
        "--daemon-url",
        " https://temper.example/ ",
        "--worker-id",
        " worker-1 ",
        "--capability",
        " ai/temper : coder ",
    ]);

    assert_eq!(config.daemon_url, "https://temper.example/");
    assert_eq!(config.worker_id, "worker-1");
    assert_eq!(
        config.capabilities,
        vec![CapabilitySpec {
            repo: "ai/temper".to_string(),
            role: "coder".to_string(),
        }]
    );
    assert_eq!(config.max_concurrent_jobs, 1);
    assert_eq!(config.poll_wait, Duration::from_millis(30_000));
    assert_eq!(config.heartbeat_interval, Duration::from_millis(10_000));
    assert_eq!(config.executor, ExecutorSelection::Stub);
}

#[test]
fn coding_executor_requires_all_coding_flags() {
    for missing_flag in ["--workspace-root", "--git-base-url", "--agent-command"] {
        let mut args = vec![
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "coding",
        ];
        if missing_flag != "--workspace-root" {
            args.extend(["--workspace-root", "/workspaces"]);
        }
        if missing_flag != "--git-base-url" {
            args.extend(["--git-base-url", "https://forgejo.example"]);
        }
        if missing_flag != "--agent-command" {
            args.extend(["--agent-command", "anvil-native"]);
        }

        let error = parse_err(&args);
        assert!(
            error.contains(missing_flag),
            "unexpected error for missing {missing_flag}: {error}"
        );
    }
}

#[test]
fn rejects_bogus_executor_and_coding_flags_with_stub_executor() {
    assert!(
        parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "bogus",
        ])
        .contains("--executor must be")
    );

    for flag in [
        "--workspace-root",
        "--git-base-url",
        "--agent-command",
        "--agent-arg",
    ] {
        let error = parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            flag,
            "value",
        ]);
        assert!(
            error.contains(&format!("{flag} requires --executor coding")),
            "unexpected error for {flag}: {error}"
        );
    }
}

#[test]
fn singleton_flags_use_last_value_and_numeric_overrides() {
    let config = parse_ok(&[
        "--daemon-url",
        "http://old.example",
        "--daemon-url",
        "http://new.example",
        "--worker-id",
        "old-worker",
        "--worker-id",
        "new-worker",
        "--capability",
        "ai/temper:coder",
        "--max-concurrent",
        "2",
        "--poll-wait-ms",
        "500",
        "--heartbeat-interval-ms",
        "250",
    ]);

    assert_eq!(config.daemon_url, "http://new.example");
    assert_eq!(config.worker_id, "new-worker");
    assert_eq!(config.max_concurrent_jobs, 2);
    assert_eq!(config.poll_wait, Duration::from_millis(500));
    assert_eq!(config.heartbeat_interval, Duration::from_millis(250));
}

#[test]
fn repeated_capabilities_are_deduplicated_preserving_order() {
    let config = parse_ok(&[
        "--daemon-url",
        "http://daemon.example",
        "--worker-id",
        "worker-1",
        "--capability",
        "ai/temper:coder",
        "--capability",
        " ai/temper : coder ",
        "--capability",
        "ai/smith:engineer",
        "--capability",
        "ai/temper:architect",
    ]);

    assert_eq!(
        config.capabilities,
        vec![
            CapabilitySpec {
                repo: "ai/temper".to_string(),
                role: "coder".to_string(),
            },
            CapabilitySpec {
                repo: "ai/smith".to_string(),
                role: "engineer".to_string(),
            },
            CapabilitySpec {
                repo: "ai/temper".to_string(),
                role: "architect".to_string(),
            },
        ]
    );
}

#[test]
fn rejects_malformed_capabilities() {
    for capability in ["nope", "ai/temper", ":role", "ai/temper:"] {
        let error = parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            capability,
        ]);
        assert!(
            error.contains("invalid --capability"),
            "unexpected error for {capability:?}: {error}"
        );
    }
}

#[test]
fn rejects_missing_required_flags() {
    assert!(parse_err(&[]).contains("--daemon-url is required"));
    assert!(
        parse_err(&["--daemon-url", "http://daemon.example"]).contains("--worker-id is required")
    );
    assert!(
        parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
        ])
        .contains("--capability is required")
    );
    assert!(
        parse_err(&[
            "--daemon-url",
            " ",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
        ])
        .contains("--daemon-url must not be empty")
    );
}

#[test]
fn rejects_zero_and_invalid_numerics() {
    for (flag, value) in [
        ("--max-concurrent", "0"),
        ("--max-concurrent", "nope"),
        ("--poll-wait-ms", "0"),
        ("--poll-wait-ms", "1.5"),
        ("--heartbeat-interval-ms", "0"),
        ("--heartbeat-interval-ms", "NaN"),
    ] {
        let error = parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            flag,
            value,
        ]);
        assert!(error.contains(flag), "unexpected error for {flag}: {error}");
    }
}

#[test]
fn rejects_unknown_flags_positionals_and_missing_values() {
    assert!(
        parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--unknown",
        ])
        .contains("unknown flag")
    );
    assert!(parse_err(&["positional"]).contains("unexpected positional argument"));
    assert!(parse_err(&["--daemon-url"]).contains("--daemon-url requires a value"));
    assert!(parse_err(&["--daemon-url", "--worker-id"]).contains("--daemon-url requires a value"));
}

#[test]
fn help_anywhere_returns_help_before_validation() {
    assert_eq!(
        parse(["--help".to_string()]).expect("help parses"),
        ParseOutcome::Help
    );
    assert_eq!(
        parse(["--daemon-url".to_string(), "-h".to_string()]).expect("help parses"),
        ParseOutcome::Help
    );
    assert!(
        parse([
            "--executor".to_string(),
            "coding".to_string(),
            "--agent-arg".to_string(),
            "--help".to_string(),
        ])
        .expect_err("agent arg help-looking value is not global help")
        .contains("--daemon-url is required")
    );
}
