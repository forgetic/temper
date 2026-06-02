use std::path::PathBuf;
use std::time::Duration;

use temper_forge::RepositoryPath;

use crate::product_chat_args::*;

fn env(key: &str) -> Option<String> {
    match key {
        HUMAN_TOKEN_ENV => Some("human-secret".into()),
        PRODUCT_MANAGER_TOKEN_ENV => Some("pm-secret".into()),
        _ => None,
    }
}

#[test]
fn product_chat_args_parse_repl_and_redact_tokens_in_debug() {
    let outcome = parse_with_env(
        [
            "repl",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
            "--auth",
            "chatgpt-oauth",
            "--codex-model",
            "gpt-5.5",
            "--auth-file",
            "/tmp/auth.json",
            "--transcript-issue",
            "3",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .expect("parses");
    let ParseOutcome::Repl(args) = outcome else {
        panic!("expected repl")
    };
    assert_eq!(args.repo, RepositoryPath::new("ai", "temper"));
    assert_eq!(args.auth, AuthKind::ChatGptOAuth);
    assert_eq!(args.transcript_issue, Some(3));
    let debug = format!("{args:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("human-secret"));
    assert!(!debug.contains("pm-secret"));
}

#[test]
fn product_chat_args_default_auth_comes_from_env_then_chatgpt() {
    let outcome = parse_with_env(
        [
            "repl",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        |key| match key {
            HUMAN_TOKEN_ENV => Some("human-secret".into()),
            PRODUCT_MANAGER_TOKEN_ENV => Some("pm-secret".into()),
            AGENTS_AUTH_ENV => Some("anthropic-oauth".into()),
            _ => None,
        },
    )
    .expect("parses");
    let ParseOutcome::Repl(args) = outcome else {
        panic!("expected repl")
    };
    assert_eq!(args.auth, AuthKind::AnthropicOAuth);
}

#[test]
fn product_chat_args_parse_process_responder_flags_and_redact_debug() {
    let outcome = parse_with_env(
        [
            "repl",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
            "--responder-command",
            "/opt/respond",
            "--responder-arg",
            "--profile",
            "--responder-arg",
            "product-manager",
            "--responder-env",
            "PM_API_KEY",
            "--responder-cwd",
            "/srv/profile",
            "--responder-timeout-secs",
            "5",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .expect("parses");
    let ParseOutcome::Repl(args) = outcome else {
        panic!("expected repl")
    };
    let responder = args.process_responder.as_ref().expect("process responder");
    assert_eq!(responder.program, PathBuf::from("/opt/respond"));
    assert_eq!(responder.args, vec!["--profile", "product-manager"]);
    assert_eq!(responder.env_allowlist, vec!["PM_API_KEY"]);
    assert_eq!(responder.timeout, Duration::from_secs(5));
    let debug = format!("{args:?}");
    assert!(debug.contains("<configured>"));
    assert!(!debug.contains("PM_API_KEY"));
}

#[test]
fn product_chat_args_parse_process_responder_from_env() {
    let outcome = parse_with_env(
        [
            "serve",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        |key| match key {
            PROCESS_RESPONDER_COMMAND_ENV => Some("/opt/respond".into()),
            PROCESS_RESPONDER_ARGS_ENV => Some(r#"["--profile","product-manager"]"#.into()),
            PROCESS_RESPONDER_ENV_ALLOWLIST_ENV => Some("PM_API_KEY, MODEL_NAME".into()),
            PROCESS_RESPONDER_TIMEOUT_ENV => Some("7".into()),
            other => env(other),
        },
    )
    .expect("parses");
    let ParseOutcome::Serve(args) = outcome else {
        panic!("expected serve")
    };
    let responder = args.process_responder.as_ref().expect("process responder");
    assert_eq!(responder.args, vec!["--profile", "product-manager"]);
    assert_eq!(responder.env_allowlist, vec!["PM_API_KEY", "MODEL_NAME"]);
    assert_eq!(responder.timeout, Duration::from_secs(7));
}

#[test]
fn product_chat_args_reject_missing_tokens() {
    let error = parse_with_env(
        [
            "repl",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        |_| None,
    )
    .unwrap_err();
    assert!(error.to_string().contains(HUMAN_TOKEN_ENV));
}
