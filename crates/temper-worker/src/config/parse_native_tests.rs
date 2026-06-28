use std::path::PathBuf;

use super::parse_test_support::{parse_err, parse_ok};
use super::*;

#[test]
fn parses_anvil_native_agent_surface_from_agent_args() {
    let config = parse_ok(&[
        "--daemon-url",
        "http://daemon.example",
        "--worker-id",
        "worker-1",
        "--capability",
        "ai/temper:coder",
        "--executor",
        "coding",
        "--workspace-root",
        " /var/lib/smith/workspaces ",
        "--git-base-url",
        " https://forgejo.example ",
        "--agent-command",
        " anvil-native ",
        "--agent-arg",
        "--provider",
        "--agent-arg",
        "anthropic",
        "--agent-arg",
        "--model",
        "--agent-arg",
        "claude-opus-4-8",
        "--agent-arg",
        "--max-iterations",
        "--agent-arg",
        "42",
    ]);

    assert_eq!(
        config.executor,
        ExecutorSelection::Coding(CodingSurface {
            workspace_root: PathBuf::from("/var/lib/smith/workspaces"),
            git_base_url: "https://forgejo.example".to_string(),
            agent: AgentSurface::AnvilNative(AnvilNativeAgentSurface {
                agent_program: "temper-agent".to_string(),
                provider: AgentProviderChoice::Anthropic,
                model: Some("claude-opus-4-8".to_string()),
                capture_dir: None,
                max_iterations: Some(42),
                enable_subagents: false,
            }),
        })
    );
}

#[test]
fn anvil_native_enable_subagents_flag_is_parsed() {
    let config = parse_ok(&[
        "--daemon-url",
        "http://daemon.example",
        "--worker-id",
        "worker-1",
        "--capability",
        "ai/temper:coder",
        "--executor",
        "coding",
        "--workspace-root",
        "/workspaces",
        "--git-base-url",
        "https://forgejo.example",
        "--agent-command",
        "anvil-native",
        "--agent-arg",
        "--subagents",
        "--agent-arg",
        "on",
    ]);
    let ExecutorSelection::Coding(surface) = config.executor else {
        panic!("expected coding executor");
    };
    let AgentSurface::AnvilNative(native) = surface.agent else {
        panic!("expected anvil-native agent");
    };
    assert!(native.enable_subagents);
}

#[test]
fn anvil_native_defaults_to_chatgpt_when_no_args() {
    let config = parse_ok(&[
        "--daemon-url",
        "http://daemon.example",
        "--worker-id",
        "worker-1",
        "--capability",
        "ai/temper:coder",
        "--executor",
        "coding",
        "--workspace-root",
        "/workspaces",
        "--git-base-url",
        "https://forgejo.example",
        "--agent-command",
        "anvil-native",
    ]);

    let ExecutorSelection::Coding(surface) = config.executor else {
        panic!("expected coding executor");
    };
    assert_eq!(
        surface.agent,
        AgentSurface::AnvilNative(AnvilNativeAgentSurface {
            agent_program: "temper-agent".to_string(),
            provider: AgentProviderChoice::ChatGpt,
            model: None,
            capture_dir: None,
            max_iterations: None,
            enable_subagents: false,
        })
    );
}

#[test]
fn non_native_agent_command_is_external_passthrough() {
    let config = parse_ok(&[
        "--daemon-url",
        "http://daemon.example",
        "--worker-id",
        "worker-1",
        "--capability",
        "ai/temper:coder",
        "--executor",
        "coding",
        "--workspace-root",
        "/workspaces",
        "--git-base-url",
        "https://forgejo.example",
        "--agent-command",
        "/opt/greeting-coder.sh",
        "--agent-arg",
        "--verbose",
    ]);

    let ExecutorSelection::Coding(surface) = config.executor else {
        panic!("expected coding executor");
    };
    assert_eq!(
        surface.agent,
        AgentSurface::ExternalCommand(vec![
            "/opt/greeting-coder.sh".to_string(),
            "--verbose".to_string(),
        ])
    );
}

#[test]
fn anvil_native_rejects_unknown_arg() {
    let error = parse_err(&[
        "--daemon-url",
        "http://daemon.example",
        "--worker-id",
        "worker-1",
        "--capability",
        "ai/temper:coder",
        "--executor",
        "coding",
        "--workspace-root",
        "/workspaces",
        "--git-base-url",
        "https://forgejo.example",
        "--agent-command",
        "anvil-native",
        "--agent-arg",
        "--verbose",
    ]);
    assert!(
        error.contains("unknown anvil-native agent arg `--verbose`"),
        "unexpected error: {error}"
    );
}
