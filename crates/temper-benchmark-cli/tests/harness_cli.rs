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
