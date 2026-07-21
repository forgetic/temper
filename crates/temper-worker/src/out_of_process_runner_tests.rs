use std::path::{Path, PathBuf};

use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1, TRACE_POLICY_FLAG};
use temper_protocol_agent::{
    AGENT_LIFECYCLE_ADDRESS_FLAG, AgentRuntimeLimitsV1, AgentToolConfig, PROVIDER_CREDENTIALS_ENV,
    RUNTIME_LIMITS_FLAG, TOOL_CONFIG_FLAG, WorkspaceContext,
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
    let outcome =
        temper_worker_io::block_on(async move { runner.run("job-test", &context, &cwd).await });
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
fn runtime_limits_flag_and_file_are_first_party_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let args_path = temp.path().join("args.txt");
    let tool_path = temp.path().join("tool-config-copy.json");
    let limits_path = temp.path().join("runtime-limits-copy.json");
    let limits = AgentRuntimeLimitsV1 {
        tool_timeout_secs: 41,
        model_connect_timeout_secs: 17,
        model_idle_timeout_secs: 13,
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
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
                "TEMPER_RUNTIME_LIMITS_OUT".to_string(),
                limits_path.display().to_string(),
            ),
        ])
        .with_runtime_limits(Some(limits));
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run("job-test", &context, &cwd).await })
        .expect("agent run succeeds");

    let args = std::fs::read_to_string(args_path).expect("args captured");
    assert!(args.lines().any(|arg| arg == RUNTIME_LIMITS_FLAG), "{args}");
    assert!(
        args.lines().any(|arg| arg == AGENT_LIFECYCLE_ADDRESS_FLAG),
        "{args}"
    );
    let copied = AgentRuntimeLimitsV1::from_json(
        &std::fs::read_to_string(limits_path).expect("runtime limits copied"),
    )
    .expect("runtime limits parse");
    assert_eq!(copied, limits);

    let (args, _) = run_fake_agent_with_tool_config("architect", None)
        .expect("third-party-compatible run succeeds");
    assert!(!args.iter().any(|arg| arg == RUNTIME_LIMITS_FLAG));
    assert!(!args.iter().any(|arg| arg == AGENT_LIFECYCLE_ADDRESS_FLAG));
}

#[test]
#[cfg(unix)]
fn trace_policy_flag_and_validated_file_are_passed_when_configured() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let args_path = temp.path().join("args.txt");
    let tool_path = temp.path().join("tool-config-copy.json");
    let trace_path = temp.path().join("trace-policy-copy.json");
    let policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Diagnostic,
        capture_thinking: true,
        ..Default::default()
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
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
                "TEMPER_TRACE_POLICY_OUT".to_string(),
                trace_path.display().to_string(),
            ),
        ])
        .with_trace_policy(Some(policy.clone()));
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run("job-test", &context, &cwd).await })
        .expect("agent run succeeds");

    let args = std::fs::read_to_string(args_path).expect("args captured");
    assert!(args.lines().any(|arg| arg == TRACE_POLICY_FLAG), "{args}");
    let copied: AgentActivityCapturePolicyV1 =
        serde_json::from_slice(&std::fs::read(trace_path).expect("trace policy copied"))
            .expect("trace policy parses");
    copied.validate().expect("trace policy validates");
    assert_eq!(copied, policy);
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
    temper_worker_io::block_on(async move { runner.run("job-test", &context, &cwd).await })
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

#[test]
#[cfg(unix)]
fn artifact_context_bundle_survives_split_process_context_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let context_copy = temp.path().join("context-copy.json");
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()]).with_env(vec![
        (
            "TEMPER_ARGS_OUT".to_string(),
            temp.path().join("args.txt").display().to_string(),
        ),
        (
            "TEMPER_TOOL_OUT".to_string(),
            temp.path()
                .join("tool-config-copy.json")
                .display()
                .to_string(),
        ),
        (
            "TEMPER_CONTEXT_OUT".to_string(),
            context_copy.display().to_string(),
        ),
    ]);
    let mut context = test_context_for_role("tester");
    context.artifact_context = Some(
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "repository": {"id":"repo-1", "path":"acme/svc"},
            "artifact_type": "issue",
            "primary": {
                "artifact":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"issue","number":7},
                "title":"Validation plan","body":"plan body","labels":["plan"],"state":"open","workflow_kind":"plan"
            },
            "lineage": [{
                "artifact":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"issue","number":1},
                "title":"Feature root","body":"feature body","labels":["feature"],"state":"open","workflow_kind":"feature"
            }],
            "validation_scope": [{
                "artifact":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"pull_request","number":9},
                "title":"Implementation PR","labels":["implementation"],"state":"open","workflow_kind":"implementation_pr",
                "relation_type":"related",
                "source":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"issue","number":7}
            }],
            "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
        }))
        .expect("artifact bundle parses"),
    );
    let expected = context.artifact_context.clone();
    let cwd = temp.path().to_path_buf();

    temper_worker_io::block_on(async move { runner.run("job-test", &context, &cwd).await })
        .expect("split-process agent run succeeds");

    let copied: WorkspaceContext = serde_json::from_slice(
        &std::fs::read(context_copy).expect("split-process context was copied"),
    )
    .expect("copied workspace context parses");
    assert_eq!(copied.artifact_context, expected);
}

#[test]
#[cfg(unix)]
fn forge_side_channel_binds_job_and_supports_repeated_indirect_reads() {
    use std::sync::{Arc, Mutex};
    use temper_protocol_agent::{ForgeContextOperation, ForgeContextResult};

    let temp = tempfile::tempdir().expect("tempdir");
    let script = forge_agent_script(temp.path());
    let responses_path = temp.path().join("forge-responses.json");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_host = Arc::clone(&observed);
    let host: crate::AgentForgeContextHost = Arc::new(move |job_id, attempt_id, operation| {
        observed_for_host
            .lock()
            .expect("observed Forge calls")
            .push((job_id, attempt_id, operation.clone()));
        Box::pin(async move {
            let value = match operation {
                ForgeContextOperation::ForgeGetItem(_) => serde_json::json!({
                    "result":"item",
                    "item": {
                        "artifact":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"issue","number":7},
                        "title":"Root", "body":"root body", "state":"open"
                    },
                    "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
                }),
                ForgeContextOperation::ForgeListRelated(_) => serde_json::json!({
                    "result":"related",
                    "root":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"issue","number":7},
                    "items":[{"artifact":{"repository":{"id":"repo-1","path":"acme/svc"},"artifact_type":"issue","number":3},"title":"Parent","state":"open"}],
                    "edges":[],
                    "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
                }),
            };
            Ok(serde_json::from_value::<ForgeContextResult>(value).expect("test Forge result"))
        })
    });
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![(
            "TEMPER_FORGE_RESPONSES_OUT".to_string(),
            responses_path.display().to_string(),
        )])
        .with_forge_context_host(host);
    let context = test_context_for_role("reviewer");
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run("host-job-284", &context, &cwd).await })
        .expect("agent run with repeated Forge reads succeeds");

    let calls = observed.lock().expect("observed Forge calls");
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|(job_id, attempt_id, _)| job_id == "host-job-284"
                && attempt_id == "host-job-284")
    );
    assert!(matches!(calls[0].2, ForgeContextOperation::ForgeGetItem(_)));
    assert!(matches!(
        calls[1].2,
        ForgeContextOperation::ForgeListRelated(_)
    ));
    let responses: serde_json::Value =
        serde_json::from_slice(&std::fs::read(responses_path).expect("captured Forge responses"))
            .expect("responses parse");
    assert_eq!(responses[0]["status"], "success");
    assert_eq!(responses[1]["result"]["items"][0]["artifact"]["number"], 3);
}

#[cfg(unix)]
fn forge_agent_script(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("forge-agent.sh");
    std::fs::write(
        &path,
        r#"#!/bin/sh
set -eu
forge=""
result=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --forge-context-address) forge="$1"; shift ;;
    --result) result="$1"; shift ;;
    --context|--workspace|--submit-for-pr-address|--tool-config|--provider|--model|--investigate-model|--provider-url|--max-iterations|--subagents|--capture-dir) shift ;;
  esac
done
python3 - "$forge" "${TEMPER_FORGE_RESPONSES_OUT:?}" <<'PY'
import json, socket, sys
address, output = sys.argv[1:]
host, port = address.rsplit(':', 1)
requests = [
  {"protocol_version":1,"operation":{"operation":"forge_get_item","repo":"acme/svc","number":7,"type":"issue","include_comments":False}},
  {"protocol_version":1,"operation":{"operation":"forge_list_related","repo":"acme/svc","number":7,"type":"issue","relations":["parent"],"depth":1,"limit":10}},
]
responses = []
for request in requests:
    stream = socket.create_connection((host, int(port)), timeout=5)
    stream.sendall(json.dumps(request).encode())
    stream.shutdown(socket.SHUT_WR)
    chunks = []
    while True:
        chunk = stream.recv(65536)
        if not chunk: break
        chunks.append(chunk)
    responses.append(json.loads(b''.join(chunks)))
    stream.close()
with open(output, 'w') as target: json.dump(responses, target)
PY
printf '{"summary":"ok"}' > "$result"
"#,
    )
    .expect("write Forge fake agent");
    let mut permissions = std::fs::metadata(&path)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod Forge fake agent");
    path
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
    temper_worker_io::block_on(async move { runner.run("job-test", &context, &cwd).await })?;

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
pub(super) fn fake_agent_script(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("fake-agent.sh");
    std::fs::write(
        &path,
        r#"#!/bin/sh
set -eu
args_out="${TEMPER_ARGS_OUT:?}"
tool_out="${TEMPER_TOOL_OUT:?}"
trace_policy_out="${TEMPER_TRACE_POLICY_OUT:-}"
runtime_limits_out="${TEMPER_RUNTIME_LIMITS_OUT:-}"
credential_out="${TEMPER_CREDENTIAL_OUT:-}"
context_out="${TEMPER_CONTEXT_OUT:-}"
: > "$args_out"
if [ -n "$credential_out" ]; then
  printf '%s' "${TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON:-}" > "$credential_out"
fi
result=""
tool=""
trace_policy=""
runtime_limits=""
context=""
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
    --trace-policy)
      trace_policy="$1"
      printf '%s\n' "$1" >> "$args_out"
      shift
      ;;
    --runtime-limits)
      runtime_limits="$1"
      printf '%s\n' "$1" >> "$args_out"
      shift
      ;;
    --activity-address)
      printf '%s\n' "$1" >> "$args_out"
      shift
      ;;
    --context)
      context="$1"
      printf '%s\n' "$1" >> "$args_out"
      shift
      ;;
    --workspace|--submit-for-pr-address|--provider|--model|--investigate-model|--provider-url|--max-iterations|--subagents|--capture-dir)
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
if [ -n "$trace_policy" ] && [ -n "$trace_policy_out" ]; then
  cp "$trace_policy" "$trace_policy_out"
fi
if [ -n "$runtime_limits" ] && [ -n "$runtime_limits_out" ]; then
  cp "$runtime_limits" "$runtime_limits_out"
fi
if [ -n "$context_out" ]; then
  cp "$context" "$context_out"
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

pub(super) fn test_context() -> WorkspaceContext {
    test_context_for_role("engineer")
}

pub(super) fn test_context_for_role(role: &str) -> WorkspaceContext {
    use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};
    WorkspaceContext {
        trace_context: None,
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
