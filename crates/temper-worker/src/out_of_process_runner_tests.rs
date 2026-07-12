use std::path::{Path, PathBuf};

use temper_protocol_agent::{
    AgentToolConfig, PROVIDER_CREDENTIALS_ENV, TOOL_CONFIG_FLAG, WorkspaceContext,
};

use super::{OutOfProcessRunner, stderr_tail};
use crate::agent_runner::{AgentRunError, AgentRunner};

#[test]
fn stderr_tail_keeps_short_input_and_truncates_long_on_boundary() {
    assert_eq!(stderr_tail(b"short", 100), "short");
    let long = "x".repeat(5_000);
    let tail = stderr_tail(long.as_bytes(), 2_000);
    assert_eq!(tail.len(), 2_000);
}

#[test]
fn empty_command_is_a_permanent_error() {
    let runner = OutOfProcessRunner::new(Vec::new());
    let context = test_context();
    let cwd = std::env::temp_dir();
    let outcome = temper_worker_io::block_on(async move { runner.run(&context, &cwd).await });
    let error = outcome.expect_err("empty command must fail");
    assert_eq!(error.class, temper_protocol_worker::FailureClass::Permanent);
}

#[test]
#[cfg(unix)]
fn tool_config_flag_and_file_are_passed_for_matching_role() {
    let config = test_tool_config();
    let (args, copied_config) =
        run_fake_agent_with_tool_config("memory-role", Some(config.clone()))
            .expect("agent run succeeds");

    let flag = args
        .iter()
        .position(|arg| arg == TOOL_CONFIG_FLAG)
        .expect("tool-config flag is present");
    assert!(
        args.get(flag + 1).is_some(),
        "tool-config path follows flag"
    );
    assert_eq!(copied_config, Some(config));
}

#[test]
#[cfg(unix)]
fn tool_config_flag_is_omitted_for_non_matching_role() {
    let (args, copied_config) =
        run_fake_agent_with_tool_config("architect", Some(test_tool_config()))
            .expect("agent run succeeds");

    assert!(!args.iter().any(|arg| arg == TOOL_CONFIG_FLAG));
    assert_eq!(copied_config, None);
}

#[test]
#[cfg(unix)]
fn provider_credentials_are_in_env_not_argv() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let args_path = temp.path().join("args.txt");
    let tool_path = temp.path().join("tool-config-copy.json");
    let credential_path = temp.path().join("credential.txt");
    let credential_json = r#"{"type":"api-key","api_key":"sk-profile-secret"}"#;
    let runner = OutOfProcessRunner::new(vec![
        script.display().to_string(),
        "--provider".to_string(),
        "anthropic".to_string(),
        "--model".to_string(),
        "claude-profile".to_string(),
    ])
    .with_env(vec![
        (
            "TEMPER_ARGS_OUT".to_string(),
            args_path.display().to_string(),
        ),
        (
            "TEMPER_TOOL_OUT".to_string(),
            tool_path.display().to_string(),
        ),
        (
            "TEMPER_CREDENTIAL_OUT".to_string(),
            credential_path.display().to_string(),
        ),
        (
            PROVIDER_CREDENTIALS_ENV.to_string(),
            credential_json.to_string(),
        ),
    ]);
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run(&context, &cwd).await })
        .expect("agent run succeeds");

    let args = std::fs::read_to_string(&args_path).expect("args captured");
    assert!(args.contains("--provider\nanthropic\n"), "args: {args}");
    assert!(args.contains("--model\nclaude-profile\n"), "args: {args}");
    assert!(args.contains("--context\n"), "args: {args}");
    assert!(args.contains("--result\n"), "args: {args}");
    assert!(args.contains("--workspace\n"), "args: {args}");
    assert!(
        !args.contains("sk-profile-secret"),
        "provider credential leaked onto argv: {args}"
    );
    assert_eq!(
        std::fs::read_to_string(&credential_path).expect("credential captured"),
        credential_json
    );
}

#[cfg(unix)]
fn run_fake_agent_with_tool_config(
    role: &str,
    tool_config: Option<AgentToolConfig>,
) -> Result<(Vec<String>, Option<AgentToolConfig>), AgentRunError> {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let args_path = temp.path().join("args.txt");
    let copied_tool_config_path = temp.path().join("tool-config-copy.json");
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![
            (
                "TEMPER_ARGS_OUT".to_string(),
                args_path.display().to_string(),
            ),
            (
                "TEMPER_TOOL_OUT".to_string(),
                copied_tool_config_path.display().to_string(),
            ),
        ])
        .with_tool_config(tool_config);
    let context = test_context_for_role(role);
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run(&context, &cwd).await })?;

    let args = std::fs::read_to_string(&args_path)
        .expect("args captured")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let copied_config = if copied_tool_config_path.exists() {
        let raw =
            std::fs::read_to_string(&copied_tool_config_path).expect("copied tool config readable");
        Some(AgentToolConfig::from_json(&raw).expect("copied tool config parses"))
    } else {
        None
    };
    Ok((args, copied_config))
}

#[cfg(unix)]
fn fake_agent_script(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("fake-agent.sh");
    std::fs::write(
        &path,
        r#"#!/bin/sh
set -eu
args_out="${TEMPER_ARGS_OUT:?}"
tool_out="${TEMPER_TOOL_OUT:?}"
credential_out="${TEMPER_CREDENTIAL_OUT:-}"
: > "$args_out"
if [ -n "$credential_out" ]; then
  printf '%s' "${TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON:-}" > "$credential_out"
fi
result=""
tool=""
while [ "$#" -gt 0 ]; do
  arg="$1"
  printf '%s\n' "$arg" >> "$args_out"
  shift
  case "$arg" in
    --result)
      result="$1"
      printf '%s\n' "$1" >> "$args_out"
      shift
      ;;
    --tool-config)
      tool="$1"
      printf '%s\n' "$1" >> "$args_out"
      shift
      ;;
    --context|--workspace|--submit-for-pr-address|--provider|--model|--investigate-model|--provider-url|--max-iterations|--subagents|--capture-dir)
      if [ "$#" -gt 0 ]; then
        printf '%s\n' "$1" >> "$args_out"
        shift
      fi
      ;;
  esac
done
if [ -n "$tool" ]; then
  cp "$tool" "$tool_out"
fi
printf '{"summary":"ok"}' > "$result"
"#,
    )
    .expect("write fake agent script");
    let mut permissions = std::fs::metadata(&path)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod fake agent script");
    path
}

fn test_tool_config() -> AgentToolConfig {
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    };

    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Auto,
            command: "codebase-memory-mcp".to_string(),
            args: vec!["--cache".to_string(), "local".to_string()],
            roles: vec!["memory-role".to_string()],
            index: CodebaseMemoryIndex::Background,
            startup_timeout_secs: 5,
            index_timeout_secs: 30,
        }),
    }
}

fn test_context() -> WorkspaceContext {
    test_context_for_role("engineer")
}

fn test_context_for_role(role: &str) -> WorkspaceContext {
    use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};
    WorkspaceContext {
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: "acme/svc".to_string(),
            owner: "acme".to_string(),
            name: "svc".to_string(),
            default_branch: "main".to_string(),
            dir: "svc".to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("smith/engineer/issue-7".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: role.to_string(),
            queue: "code".to_string(),
            kind: "issue".to_string(),
            target: "Issue { number: ItemNumber(7) }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-7".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}
