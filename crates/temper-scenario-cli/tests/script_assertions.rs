// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
}

fn write_script_assertion_bundle(
    bundle: &Path,
    name: &str,
    script_body: &str,
    assertion_toml: &str,
) {
    std::fs::create_dir_all(bundle.join("scripts")).expect("create script assertion bundle");
    std::fs::write(bundle.join("scripts/assert-hook.sh"), script_body).expect("write hook script");
    std::fs::write(
        bundle.join("scenario.toml"),
        format!(
            "name = \"{name}\"\n\
             intent = \"Ephemeral validation bundle with a script assertion hook.\"\n\
             [fixtures]\n\
             extends = \"scenarios/basic-delivery\"\n\
             [runner]\n\
             uses = \"basic-delivery\"\n\
             {assertion_toml}\n"
        ),
    )
    .expect("write script assertion manifest");
}

fn read_json(path: &Path) -> serde_json::Value {
    let source = std::fs::read_to_string(path).expect("read json");
    serde_json::from_str(&source).expect("parse json")
}

fn assertion_result<'a>(json: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    json["assertions"]["results"]
        .as_array()
        .expect("assertion results")
        .iter()
        .find(|result| result["id"] == id)
        .unwrap_or_else(|| panic!("missing assertion result {id}: {json:#?}"))
}

#[test]
fn run_executes_successful_script_assertion_hook() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("script-hook-success");
    write_script_assertion_bundle(
        &bundle,
        "script-hook-success",
        r#"set -euo pipefail
context="${1:?context}"
test "$context" = "${TEMPER_SCENARIO_CONTEXT:?}"
test -d "${TEMPER_SCENARIO_ARTIFACT_DIR:?}"
grep -q '"runner_id": "basic-delivery"' "$context"
grep -q '"tier": "hermetic"' "$context"
grep -q '"run_evidence"' "$context"
echo "context includes runner and evidence"
"#,
        "[[assertions]]\n\
         id = \"context-has-runner\"\n\
         kind = \"command\"\n\
         command = \"scripts/assert-hook.sh\"\n\
         phase = \"after-convergence\"\n\
         timeout_ms = 5000\n",
    );
    let evidence = dir.path().join("script-success.run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("[passed] context-has-runner"), "{stdout}");
    assert!(
        stdout.contains("stdout excerpt: context includes runner and evidence"),
        "{stdout}"
    );
    let json = read_json(&evidence);
    let result = assertion_result(&json, "context-has-runner");
    assert_eq!(result["status"], "passed");
    assert_eq!(result["kind"], "command");
    assert_eq!(result["phase"], "after-convergence");
    let context_path = PathBuf::from(result["context_path"].as_str().unwrap());
    let stdout_path = PathBuf::from(result["stdout_path"].as_str().unwrap());
    assert!(context_path.is_file(), "context path: {context_path:?}");
    assert!(stdout_path.is_file(), "stdout path: {stdout_path:?}");
    assert!(
        std::fs::read_to_string(&stdout_path)
            .expect("read hook stdout")
            .contains("context includes runner and evidence")
    );
    assert!(
        json["artifacts"]["log_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str() == result["stdout_path"].as_str())
    );
}

#[test]
fn run_records_nonzero_script_assertion_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("script-hook-failure");
    write_script_assertion_bundle(
        &bundle,
        "script-hook-failure",
        "set -euo pipefail\necho 'intentional hook failure evidence' >&2\nexit 7\n",
        "[[assertions]]\n\
         id = \"script-exits-nonzero\"\n\
         kind = \"command\"\n\
         command = \"scripts/assert-hook.sh\"\n\
         timeout_ms = 5000\n",
    );
    let evidence = dir.path().join("script-failure.run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);

    assert!(!output.status.success(), "non-zero hook should fail run");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("[failed] script-exits-nonzero"), "{stdout}");
    assert!(stdout.contains("hook exited non-zero"), "{stdout}");
    assert!(
        stdout.contains("stderr excerpt: intentional hook failure evidence"),
        "{stdout}"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("manifest assertions failed"), "{stderr}");
    let json = read_json(&evidence);
    let result = assertion_result(&json, "script-exits-nonzero");
    assert_eq!(result["status"], "failed");
    assert!(
        result["exit_status"]
            .as_str()
            .is_some_and(|status| status.contains("exit code 7")),
        "{result:#?}"
    );
    let stderr_path = PathBuf::from(result["stderr_path"].as_str().unwrap());
    assert!(
        std::fs::read_to_string(stderr_path)
            .expect("read hook stderr")
            .contains("intentional hook failure evidence")
    );
}

#[test]
fn run_times_out_script_assertion_hook() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("script-hook-timeout");
    write_script_assertion_bundle(
        &bundle,
        "script-hook-timeout",
        "set -euo pipefail\nsleep 5\necho late\n",
        "[[assertions]]\n\
         id = \"script-times-out\"\n\
         kind = \"command\"\n\
         command = \"scripts/assert-hook.sh\"\n\
         timeout_ms = 100\n",
    );
    let evidence = dir.path().join("script-timeout.run-evidence.json");

    let output = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);

    assert!(!output.status.success(), "timed-out hook should fail run");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("[failed] script-times-out"), "{stdout}");
    assert!(stdout.contains("hook timed out after 100ms"), "{stdout}");
    let json = read_json(&evidence);
    let result = assertion_result(&json, "script-times-out");
    assert_eq!(result["status"], "failed");
    assert!(
        result["exit_status"]
            .as_str()
            .is_some_and(|status| status.contains("timed out after 100ms")),
        "{result:#?}"
    );
    assert_eq!(result["timeout_ms"], 100);
}

#[test]
fn run_rejects_unsafe_script_assertion_command_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("unsafe-script-hook");
    std::fs::create_dir_all(&bundle).expect("create unsafe bundle");
    std::fs::write(
        bundle.join("scenario.toml"),
        "name = \"unsafe-script-hook\"\n\
         intent = \"Ephemeral validation bundle with an unsafe hook path.\"\n\
         [fixtures]\n\
         extends = \"scenarios/basic-delivery\"\n\
         [runner]\n\
         uses = \"basic-delivery\"\n\
         [[assertions]]\n\
         id = \"unsafe-command\"\n\
         kind = \"command\"\n\
         command = \"../outside.sh\"\n",
    )
    .expect("write unsafe manifest");

    let output = temper_scenario(&["run", &bundle.to_string_lossy()]);

    assert!(!output.status.success(), "unsafe hook path should fail");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains(
            "assertions[0].command: must be a local relative path without `..` components"
        ),
        "{stderr}"
    );
}

#[test]
fn validate_workflow_retains_report_for_script_assertion_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("script-hook-workflow-failure");
    write_script_assertion_bundle(
        &bundle,
        "script-hook-workflow-failure",
        "set -euo pipefail\necho 'workflow hook failure evidence' >&2\nexit 7\n",
        "[[assertions]]\n\
         id = \"script-exits-nonzero\"\n\
         kind = \"command\"\n\
         command = \"scripts/assert-hook.sh\"\n\
         timeout_ms = 5000\n",
    );
    let output_dir = dir.path().join("validation-artifacts");

    let output = temper_scenario(&[
        "validate",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--scenario",
        &bundle.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        !output.status.success(),
        "script hook failure should fail validation workflow"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("[failed] script-exits-nonzero"), "{stdout}");
    assert!(stdout.contains("validation report:"), "{stdout}");
    assert!(stdout.contains("validation result:"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("scenario assertions failed"), "{stderr}");

    let evidence_path = output_dir.join("run-evidence.json");
    let markdown_path = output_dir.join("validation-pr-123-deadbeef.md");
    let json_path = output_dir.join("validation-pr-123-deadbeef.json");
    assert!(evidence_path.is_file(), "evidence path: {evidence_path:?}");
    assert!(markdown_path.is_file(), "markdown path: {markdown_path:?}");
    assert!(json_path.is_file(), "json path: {json_path:?}");
    assert!(
        output_dir.join("script-assertions").is_dir(),
        "script assertion artifacts should be retained"
    );

    let markdown = std::fs::read_to_string(markdown_path).expect("read report");
    assert!(markdown.contains("- Verdict: failed"), "{markdown}");
    assert!(
        markdown.contains("assertion failed `script-exits-nonzero`"),
        "{markdown}"
    );
    assert!(
        markdown.contains("workflow hook failure evidence"),
        "{markdown}"
    );
}

#[test]
fn validate_pr_renders_script_assertion_hook_results() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("script-hook-report");
    write_script_assertion_bundle(
        &bundle,
        "script-hook-report",
        "set -euo pipefail\necho 'script hook report evidence'\n",
        "[[assertions]]\n\
         id = \"context-has-runner\"\n\
         kind = \"command\"\n\
         command = \"scripts/assert-hook.sh\"\n\
         timeout_ms = 5000\n",
    );
    let evidence = dir.path().join("script-report.run-evidence.json");
    let run = temper_scenario(&[
        "run",
        "--evidence-out",
        &evidence.to_string_lossy(),
        &bundle.to_string_lossy(),
    ]);
    assert!(
        run.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let output_dir = dir.path().join("reports");

    let output = temper_scenario(&[
        "validate-pr",
        "--pr",
        "123",
        "--sha",
        "deadbeef",
        "--run-evidence",
        &evidence.to_string_lossy(),
        "--output-dir",
        &output_dir.to_string_lossy(),
    ]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let markdown = std::fs::read_to_string(PathBuf::from(stdout.trim())).expect("read report");
    assert!(
        markdown.contains("assertion passed `context-has-runner`: Script assertion hook completed successfully. kind=command phase=after-convergence"),
        "{markdown}"
    );
    assert!(
        markdown.contains("stdout excerpt: script hook report evidence"),
        "{markdown}"
    );
    assert!(markdown.contains("log path:"), "{markdown}");
}
