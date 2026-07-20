// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[cfg(unix)]
#[test]
fn run_cli_writes_harness_artifacts_and_honors_repetition_override() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let fixture = temporary.path().join("fixture/repo");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("README.md"), "# fixture\n").unwrap();
    write_context(temporary.path());
    fs::write(
        temporary.path().join("jig.json"),
        r#"{"fixed":{"text":"{}"}}"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("benchmark.toml"),
        r#"schema = "temper.benchmark.v1"
name = "cli-harness"
fixture = "fixture"
workspace_context = "context.json"
capture = "metadata"
jig_script = "jig.json"
repetitions = 3
"#,
    )
    .unwrap();

    let agent = temporary.path().join("fake-agent.sh");
    fs::write(
        &agent,
        r##"#!/bin/sh
set -eu
result=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--result" ]; then
    result="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' '{"title":"Fake harness","body":"# Report","summary":"fake completed"}' > "$result"
"##,
    )
    .unwrap();
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();

    let output_dir = temporary.path().join("artifacts");
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"));
    command
        .arg("run")
        .arg("--benchmark")
        .arg(temporary.path().join("benchmark.toml"))
        .arg("--mode")
        .arg("harness")
        .arg("--agent-bin")
        .arg(&agent)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--repetitions")
        .arg("2");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("not representative LLM performance"));

    let aggregate: Value =
        serde_json::from_slice(&fs::read(output_dir.join("aggregate.json")).unwrap()).unwrap();
    assert_eq!(aggregate["benchmark"], "cli-harness");
    assert_eq!(aggregate["mode"], "harness");
    assert_eq!(aggregate["outcomes"]["total"], 2);
    assert_eq!(
        aggregate["runs"][0]["summary"]["workspace_result"]["title"],
        "Fake harness"
    );
    for repetition in ["001", "002"] {
        let root = output_dir.join("repetitions").join(repetition);
        for artifact in [
            "manifest.toml",
            "workspace-context.json",
            "baselines.json",
            "trace.export.jsonl",
            "workspace-result.json",
            "validation.json",
            "diff.json",
            "run.json",
            "run.md",
        ] {
            assert!(
                root.join(artifact).is_file(),
                "missing {repetition}/{artifact}"
            );
        }
        assert!(
            fs::read_to_string(root.join("run.md"))
                .unwrap()
                .contains("not representative LLM performance")
        );
    }
    assert!(!output_dir.join("repetitions/003").exists());
}

#[cfg(unix)]
#[test]
fn run_cli_retains_failed_agent_evidence_and_continues_later_repetitions() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let fixture = temporary.path().join("fixture/repo");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("README.md"), "# fixture\n").unwrap();
    write_context(temporary.path());
    fs::write(
        temporary.path().join("jig.json"),
        r#"{"fixed":{"text":"{}"}}"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("benchmark.toml"),
        r#"schema = "temper.benchmark.v1"
name = "mixed-agent-outcomes"
fixture = "fixture"
workspace_context = "context.json"
capture = "metadata"
jig_script = "jig.json"
repetitions = 3
"#,
    )
    .unwrap();

    let agent = temporary.path().join("mixed-agent.sh");
    fs::write(
        &agent,
        r##"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
state="$root/invocations"
count=0
if [ -f "$state" ]; then
  count=$(cat "$state")
fi
count=$((count + 1))
printf '%s\n' "$count" > "$state"
result=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--result" ]; then
    result="$2"
    shift 2
  else
    shift
  fi
done
if [ "$count" -eq 2 ]; then
  printf '%s\n' 'intentional failing agent process' >&2
  printf '%s\n' failed > repo/failure-evidence.txt
  exit 17
fi
printf '{"title":"Completed repetition %s","body":"# Report","summary":"completed"}\n' "$count" > "$result"
"##,
    )
    .unwrap();
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();

    let output_dir = temporary.path().join("artifacts");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("run")
        .arg("--benchmark")
        .arg(temporary.path().join("benchmark.toml"))
        .arg("--mode")
        .arg("harness")
        .arg("--agent-bin")
        .arg(&agent)
        .arg("--output-dir")
        .arg(&output_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("agent repetition 2 failed"), "{stderr}");
    assert!(stderr.contains("status 17"), "{stderr}");

    let aggregate: Value =
        serde_json::from_slice(&fs::read(output_dir.join("aggregate.json")).unwrap()).unwrap();
    assert_eq!(aggregate["outcomes"]["total"], 3);
    assert_eq!(aggregate["outcomes"]["succeeded"], 2);
    assert_eq!(aggregate["outcomes"]["failed"], 1);
    assert_eq!(aggregate["outcomes"]["cancelled"], 0);
    assert_eq!(aggregate["outcomes"]["incomplete"], 0);
    assert_eq!(
        aggregate["runs"][0]["summary"]["terminal"]["status"],
        "succeeded"
    );
    assert_eq!(
        aggregate["runs"][1]["summary"]["terminal"]["status"],
        "failed"
    );
    assert_eq!(
        aggregate["runs"][1]["summary"]["terminal"]["failure"]["code"],
        "child_process"
    );
    assert!(aggregate["runs"][1]["summary"]["workspace_result"].is_null());
    assert_eq!(
        aggregate["runs"][2]["summary"]["workspace_result"]["title"],
        "Completed repetition 3"
    );

    let failed_root = output_dir.join("repetitions/002");
    for artifact in [
        "manifest.toml",
        "workspace-context.json",
        "baselines.json",
        "trace.export.jsonl",
        "validation.json",
        "diff.json",
        "run.json",
        "run.md",
    ] {
        assert!(failed_root.join(artifact).is_file(), "missing {artifact}");
    }
    assert!(!failed_root.join("workspace-result.json").exists());
    assert!(
        fs::read_to_string(failed_root.join("trace.export.jsonl"))
            .unwrap()
            .contains("run.failed")
    );
    assert!(
        fs::read_to_string(failed_root.join("run.md"))
            .unwrap()
            .contains("child_process: agent run failed with a transient error")
    );
    assert!(output_dir.join("aggregate.md").is_file());
}

#[test]
fn run_cli_rejects_live_mode_before_touching_config_credentials_or_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let output_dir = temporary.path().join("unused");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .args([
            "run",
            "--benchmark",
            "missing.toml",
            "--mode",
            "live",
            "--agent-bin",
            "missing-agent",
            "--output-dir",
        ])
        .arg(&output_dir)
        .args([
            "--config",
            "missing-config.toml",
            "--secrets",
            "missing-credentials.toml",
        ])
        .env_remove("TEMPER_BENCHMARK_LIVE")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TEMPER_BENCHMARK_LIVE=1"), "{stderr}");
    assert!(stderr.contains("no config, credentials, workspace, or provider was accessed"));
    assert!(!stderr.contains("missing-config.toml"), "{stderr}");
    assert!(!stderr.contains("missing-credentials.toml"), "{stderr}");
    assert!(!output_dir.exists());
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
                "branch_hint": "benchmark/fixture"
            }],
            "work_item": {
                "role": "engineer",
                "queue": "code_ready",
                "kind": "code",
                "target": "Issue { number: ItemNumber(1) }",
                "context": "{}"
            },
            "action": "open_pr",
            "correlation_key": "cli-harness",
            "checkout": "writable"
        }))
        .unwrap(),
    )
    .unwrap();
}
