// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const SECRET_SENTINEL: &str = "TEMPER_LIVE_SECRET_SENTINEL_550_d9d47e";

#[cfg(unix)]
#[test]
fn live_run_uses_resolved_credentials_and_excludes_them_from_every_artifact() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let fixture = temporary.path().join("fixture/repo");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("README.md"), "# live fixture\n").unwrap();
    write_context(temporary.path());
    fs::write(temporary.path().join("jig.json"), "{}\n").unwrap();
    fs::write(
        temporary.path().join("benchmark.toml"),
        r#"schema = "temper.benchmark.v1"
name = "cli-live"
fixture = "fixture"
workspace_context = "context.json"
capture = "metadata"
jig_script = "jig.json"
post_run_commands = [["sh", "-c", "cat repo/provider-output.txt"]]
repetitions = 1
"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("config.toml"),
        r#"schema_version = 1

[observability.agent_traces]
capture = "metadata"

[agent]
provider = "deepseek"
max_iterations = 9
enable_subagents = false

[agent.providers.deepseek]
models = { main = "deepseek-live-test" }
"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("credentials.toml"),
        format!(
            "schema_version = 1\n[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"{SECRET_SENTINEL}\"\n"
        ),
    )
    .unwrap();

    let agent = temporary.path().join("fake-temper-agent");
    fs::write(
        &agent,
        format!(
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
printf '%s' "${{TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON:?}}" > "${{TEMPER_TEST_RECEIVED_CREDENTIAL:?}}"
printf '%s\n' '{SECRET_SENTINEL}' >&2
printf '%s\n' '{SECRET_SENTINEL}' > repo/provider-output.txt
printf '%s\n' changed > 'repo/{SECRET_SENTINEL}'
printf '%s\n' '{{"title":"Fake live","body":"# Report","summary":"{SECRET_SENTINEL}"}}' > "$result"
"##
        ),
    )
    .unwrap();
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();

    let output_dir = temporary.path().join("artifacts");
    let credential_copy = temporary.path().join("received-credential.json");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("run")
        .arg("--benchmark")
        .arg(temporary.path().join("benchmark.toml"))
        .arg("--mode")
        .arg("live")
        .arg("--agent-bin")
        .arg(&agent)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--config")
        .arg(temporary.path().join("config.toml"))
        .arg("--secrets")
        .arg(temporary.path().join("credentials.toml"))
        .env("TEMPER_BENCHMARK_LIVE", "1")
        .env("TEMPER_TEST_RECEIVED_CREDENTIAL", &credential_copy)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(&credential_copy)
            .unwrap()
            .contains(SECRET_SENTINEL),
        "resolved credential was not passed to the child invocation"
    );

    let aggregate: Value =
        serde_json::from_slice(&fs::read(output_dir.join("aggregate.json")).unwrap()).unwrap();
    assert_eq!(aggregate["mode"], "live");
    assert_eq!(
        aggregate["runs"][0]["summary"]["workspace_result"]["summary"],
        "[REDACTED]"
    );
    let validation: Value = serde_json::from_slice(
        &fs::read(output_dir.join("repetitions/001/validation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        validation["post_run_commands"][0]["stdout_tail"],
        "[REDACTED]\n"
    );
    let diff: Value =
        serde_json::from_slice(&fs::read(output_dir.join("repetitions/001/diff.json")).unwrap())
            .unwrap();
    assert!(
        diff["repositories"][0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "[REDACTED]")
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Timing values are advisory"));

    for path in files_below(&output_dir) {
        let bytes = fs::read(&path).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SECRET_SENTINEL),
            "secret leaked into {}",
            path.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn failed_live_agent_retains_redacted_trace_summary_and_aggregate() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let fixture = temporary.path().join("fixture/repo");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("README.md"), "# live failure fixture\n").unwrap();
    write_context(temporary.path());
    fs::write(temporary.path().join("jig.json"), "{}\n").unwrap();
    fs::write(
        temporary.path().join("benchmark.toml"),
        r#"schema = "temper.benchmark.v1"
name = "cli-live-failure"
fixture = "fixture"
workspace_context = "context.json"
capture = "metadata"
jig_script = "jig.json"
post_run_commands = [["sh", "-c", "cat repo/provider-output.txt"]]
repetitions = 1
"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("config.toml"),
        r#"schema_version = 1

[observability.agent_traces]
capture = "metadata"

[agent]
provider = "deepseek"
max_iterations = 9
enable_subagents = false

[agent.providers.deepseek]
models = { main = "deepseek-live-failure-test" }
"#,
    )
    .unwrap();
    fs::write(
        temporary.path().join("credentials.toml"),
        format!(
            "schema_version = 1\n[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"{SECRET_SENTINEL}\"\n"
        ),
    )
    .unwrap();

    let agent = temporary.path().join("failing-temper-agent");
    fs::write(
        &agent,
        format!(
            r##"#!/bin/sh
set -eu
printf '%s\n' '{SECRET_SENTINEL}' >&2
printf '%s\n' '{SECRET_SENTINEL}' > repo/provider-output.txt
printf '%s\n' changed > 'repo/{SECRET_SENTINEL}'
exit 23
"##
        ),
    )
    .unwrap();
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();

    let output_dir = temporary.path().join("artifacts");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("run")
        .arg("--benchmark")
        .arg(temporary.path().join("benchmark.toml"))
        .arg("--mode")
        .arg("live")
        .arg("--agent-bin")
        .arg(&agent)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--config")
        .arg(temporary.path().join("config.toml"))
        .arg("--secrets")
        .arg(temporary.path().join("credentials.toml"))
        .env("TEMPER_BENCHMARK_LIVE", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET_SENTINEL));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET_SENTINEL));

    let aggregate: Value =
        serde_json::from_slice(&fs::read(output_dir.join("aggregate.json")).unwrap()).unwrap();
    assert_eq!(aggregate["outcomes"]["total"], 1);
    assert_eq!(aggregate["outcomes"]["failed"], 1);
    assert_eq!(
        aggregate["runs"][0]["summary"]["terminal"]["status"],
        "failed"
    );
    assert!(aggregate["runs"][0]["summary"]["workspace_result"].is_null());

    let repetition = output_dir.join("repetitions/001");
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
        assert!(repetition.join(artifact).is_file(), "missing {artifact}");
    }
    assert!(!repetition.join("workspace-result.json").exists());
    let validation: Value =
        serde_json::from_slice(&fs::read(repetition.join("validation.json")).unwrap()).unwrap();
    assert_eq!(
        validation["post_run_commands"][0]["stdout_tail"],
        "[REDACTED]\n"
    );
    let diff: Value =
        serde_json::from_slice(&fs::read(repetition.join("diff.json")).unwrap()).unwrap();
    assert!(
        diff["repositories"][0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "[REDACTED]")
    );
    for path in files_below(&output_dir) {
        let bytes = fs::read(&path).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SECRET_SENTINEL),
            "secret leaked into {}",
            path.display()
        );
    }
}

#[test]
fn live_run_rejects_third_party_profile_supervision_before_workspace_preparation() {
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("config.toml");
    fs::write(
        &config,
        r#"schema_version = 1

[[worker.pools]]
name = "engineers"
roles = ["engineer"]
repos = ["acme/fixture"]
max_concurrent_jobs = 1
agent_profile = "vendor"

[agent.profiles.vendor]
command = ["vendor-agent"]
"#,
    )
    .unwrap();
    let output_dir = temporary.path().join("artifacts");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("run")
        .args(["--benchmark", "missing-benchmark.toml", "--mode", "live"])
        .arg("--agent-bin")
        .arg(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--config")
        .arg(&config)
        .args(["--pool", "engineers"])
        .env("TEMPER_BENCHMARK_LIVE", "1")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("require first-party Temper supervision"),
        "{stderr}"
    );
    assert!(!stderr.contains("missing-benchmark.toml"), "{stderr}");
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

fn files_below(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files
}
