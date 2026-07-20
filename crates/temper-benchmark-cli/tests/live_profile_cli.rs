// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[cfg(unix)]
#[test]
fn live_run_uses_supplied_agent_binary_with_explicit_first_party_profile() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let fixture = temporary.path().join("fixture/repo");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("README.md"), "# profiled live fixture\n").unwrap();
    write_context(temporary.path());
    fs::write(temporary.path().join("jig.json"), "{}\n").unwrap();
    fs::write(
        temporary.path().join("benchmark.toml"),
        r#"schema = "temper.benchmark.v1"
name = "profiled-cli-live"
fixture = "fixture"
workspace_context = "context.json"
capture = "metadata"
jig_script = "jig.json"
repetitions = 1
"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("config.toml"),
        r#"schema_version = 1

[worker]
max_no_progress_secs = 900
max_run_secs = 120

[[worker.pools]]
name = "engineers"
roles = ["engineer"]
repos = ["acme/fixture"]
max_concurrent_jobs = 1
agent_profile = "benchmark"

[observability.agent_traces]
capture = "diagnostic"
retention_days = 19
max_run_bytes = 123456789

[agent.tools.codebase_memory]
mode = "required"
command = "profile-codebase-memory"
args = ["--profile-test"]
roles = ["engineer"]
index = "blocking"
startup_timeout_secs = 7
index_timeout_secs = 23

[agent.profiles.benchmark]
command = ["temper", "agent"]
provider = "deepseek"
model = "profile-main-model"
investigate_model = "profile-investigate-model"
provider_url = "https://profile-provider.invalid/v1"
max_iterations = 17
subagents = false
credential = "profile-provider-credentials"

[agent.profiles.benchmark.deadlines]
tool_timeout_secs = 31
model_connect_timeout_secs = 29
model_idle_timeout_secs = 27
"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("credentials.toml"),
        r#"schema_version = 1

[secrets.profile-provider-credentials]
kind = "provider-credentials"
provider = "deepseek"
auth = "api-key"
api_key = "profile-live-secret"
"#,
    )
    .unwrap();

    let profile_bin_dir = temporary.path().join("profile-bin");
    fs::create_dir(&profile_bin_dir).unwrap();
    let profile_agent = profile_bin_dir.join("temper");
    fs::write(
        &profile_agent,
        r#"#!/bin/sh
set -eu
: > "${TEMPER_PROFILE_AGENT_MARKER:?}"
exit 97
"#,
    )
    .unwrap();
    fs::set_permissions(&profile_agent, fs::Permissions::from_mode(0o755)).unwrap();

    let supplied_agent = temporary.path().join("supplied-temper-agent");
    fs::write(
        &supplied_agent,
        r##"#!/bin/sh
set -eu
: > "${TEMPER_SUPPLIED_AGENT_MARKER:?}"
printf '%s\n' "$@" > "${TEMPER_SUPPLIED_ARGS:?}"
printf '%s' "${TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON:?}" > "${TEMPER_SUPPLIED_CREDENTIALS:?}"
result=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --result) result="$2"; shift 2 ;;
    --tool-config) cp "$2" "${TEMPER_SUPPLIED_TOOL_CONFIG:?}"; shift 2 ;;
    --runtime-limits) cp "$2" "${TEMPER_SUPPLIED_RUNTIME_LIMITS:?}"; shift 2 ;;
    --trace-policy) cp "$2" "${TEMPER_SUPPLIED_TRACE_POLICY:?}"; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s\n' '{"title":"Profiled live","body":"# Report","summary":"supplied agent completed"}' > "$result"
"##,
    )
    .unwrap();
    fs::set_permissions(&supplied_agent, fs::Permissions::from_mode(0o755)).unwrap();

    let profile_marker = temporary.path().join("profile-agent-ran");
    let supplied_marker = temporary.path().join("supplied-agent-ran");
    let supplied_args = temporary.path().join("supplied-args");
    let supplied_credentials = temporary.path().join("supplied-credentials");
    let supplied_tool_config = temporary.path().join("supplied-tool-config.json");
    let supplied_runtime_limits = temporary.path().join("supplied-runtime-limits.json");
    let supplied_trace_policy = temporary.path().join("supplied-trace-policy.json");
    let path = std::env::join_paths(std::iter::once(profile_bin_dir.clone()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let output_dir = temporary.path().join("artifacts");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("run")
        .arg("--benchmark")
        .arg(temporary.path().join("benchmark.toml"))
        .args(["--mode", "live"])
        .arg("--agent-bin")
        .arg(&supplied_agent)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--config")
        .arg(temporary.path().join("config.toml"))
        .arg("--secrets")
        .arg(temporary.path().join("credentials.toml"))
        .args(["--pool", "engineers"])
        .env("TEMPER_BENCHMARK_LIVE", "1")
        .env("PATH", path)
        .env("TEMPER_PROFILE_AGENT_MARKER", &profile_marker)
        .env("TEMPER_SUPPLIED_AGENT_MARKER", &supplied_marker)
        .env("TEMPER_SUPPLIED_ARGS", &supplied_args)
        .env("TEMPER_SUPPLIED_CREDENTIALS", &supplied_credentials)
        .env("TEMPER_SUPPLIED_TOOL_CONFIG", &supplied_tool_config)
        .env("TEMPER_SUPPLIED_RUNTIME_LIMITS", &supplied_runtime_limits)
        .env("TEMPER_SUPPLIED_TRACE_POLICY", &supplied_trace_policy)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        supplied_marker.is_file(),
        "supplied --agent-bin did not run"
    );
    assert!(
        !profile_marker.exists(),
        "the profile command ran instead of --agent-bin"
    );

    let args = fs::read_to_string(&supplied_args).unwrap();
    let args: Vec<_> = args.lines().collect();
    for expected in [
        ["--provider", "deepseek"],
        ["--model", "profile-main-model"],
        ["--investigate-model", "profile-investigate-model"],
        ["--provider-url", "https://profile-provider.invalid/v1"],
        ["--max-iterations", "17"],
        ["--subagents", "off"],
    ] {
        assert!(
            args.windows(2).any(|pair| pair == expected),
            "missing profile arguments {expected:?} in {args:?}"
        );
    }
    assert!(args.contains(&"--tool-config"));
    assert!(args.contains(&"--runtime-limits"));
    assert!(args.contains(&"--trace-policy"));

    let credentials: Value =
        serde_json::from_slice(&fs::read(supplied_credentials).unwrap()).unwrap();
    assert_eq!(credentials["api_key"], "profile-live-secret");
    let tool_config: Value =
        serde_json::from_slice(&fs::read(supplied_tool_config).unwrap()).unwrap();
    assert_eq!(tool_config["codebase_memory"]["mode"], "required");
    assert_eq!(
        tool_config["codebase_memory"]["command"],
        "profile-codebase-memory"
    );
    let runtime_limits: Value =
        serde_json::from_slice(&fs::read(supplied_runtime_limits).unwrap()).unwrap();
    assert_eq!(runtime_limits["tool_timeout_secs"], 31);
    assert_eq!(runtime_limits["model_connect_timeout_secs"], 29);
    assert_eq!(runtime_limits["model_idle_timeout_secs"], 27);
    let trace_policy: Value =
        serde_json::from_slice(&fs::read(supplied_trace_policy).unwrap()).unwrap();
    assert_eq!(trace_policy["capture"], "metadata");
    assert_eq!(trace_policy["retention_days"], 19);
    assert_eq!(trace_policy["max_run_bytes"], 123456789);
}

fn write_context(root: &Path) {
    fs::write(
        root.join("context.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "repos": [{
                "id": "repo-1",
                "owner": "acme",
                "name": "fixture",
                "default_branch": "main",
                "dir": "repo",
                "access": "writable",
                "base_branch": "main",
                "branch_hint": "benchmark/live"
            }],
            "work_item": {
                "role": "engineer",
                "queue": "code_ready",
                "kind": "code",
                "target": "Issue { number: ItemNumber(550) }",
                "context": "{}"
            },
            "action": "open_pr",
            "correlation_key": "cli-live",
            "checkout": "writable"
        }))
        .unwrap(),
    )
    .unwrap();
}
