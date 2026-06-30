// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temper_scenario(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .args(args)
        .output()
        .expect("run temper-scenario")
}

fn write_valid_scenario(root: &Path, name: &str, status: &str, stability: &str, intent: &str) {
    let scenario = root.join(name);
    std::fs::create_dir_all(scenario.join("repo")).expect("create repo fixture");
    std::fs::write(scenario.join("README.md"), "scenario docs").expect("write readme");
    std::fs::write(
        scenario.join("scenario.toml"),
        format!(
            "schema_version = 1\n\
             name = \"{name}\"\n\
             status = \"{status}\"\n\
             stability = \"{stability}\"\n\
             intent = \"{intent}\"\n\
             files = [\"README.md\"]\n\
             [[repositories]]\n\
             id = \"primary\"\n\
             repo = \"ai/temper\"\n\
             path = \"repo\"\n\
             [[issues]]\n\
             repo = \"primary\"\n\
             number = 37\n"
        ),
    )
    .expect("write manifest");
}

#[test]
fn help_is_concise_and_successful() {
    let output = temper_scenario(&["check", "--help"]);

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper-scenario check [PATH]"),
        "{stdout}"
    );
    assert!(stdout.contains("path: error: field: message"), "{stdout}");
}

#[test]
fn list_prints_stable_tab_separated_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenarios = dir.path().join("scenarios");
    write_valid_scenario(
        &scenarios,
        "alpha",
        "ready",
        "experimental",
        "Exercise delivery.",
    );

    let output = temper_scenario(&["list", &scenarios.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let expected = format!(
        "name\tstatus\tstability\tintent\tpath\nalpha\tready\texperimental\tExercise delivery.\t{}\n",
        scenarios.join("alpha").display()
    );
    assert_eq!(stdout, expected);
}

#[test]
fn check_succeeds_for_all_valid_scenarios() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenarios = dir.path().join("scenarios");
    write_valid_scenario(&scenarios, "alpha", "ready", "experimental", "one");
    write_valid_scenario(&scenarios, "beta", "draft", "unstable", "two");

    let output = temper_scenario(&["check", &scenarios.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "OK - checked 2 scenario(s).\n");
}

#[test]
fn check_succeeds_for_checked_in_basic_delivery_manifest() {
    let scenario = workspace_root().join("scenarios/basic-delivery");

    let output = temper_scenario(&["check", &scenario.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout utf8"),
        "OK - checked 1 scenario(s).\n"
    );
}

#[test]
fn list_succeeds_for_checked_in_scenarios_directory() {
    let scenarios = workspace_root().join("scenarios");

    let output = temper_scenario(&["list", &scenarios.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("basic-delivery\tactive\tprovisional\tExercise the minimal"),
        "{stdout}"
    );
}

#[test]
fn run_succeeds_for_checked_in_basic_delivery_scenario() {
    let scenario = workspace_root().join("scenarios/basic-delivery");

    let output = temper_scenario(&["run", &scenario.to_string_lossy()]);

    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("scenario: basic-delivery"), "{stdout}");
    assert!(stdout.contains("verdict: passed"), "{stdout}");
    assert!(stdout.contains("seeded issue: #"), "{stdout}");
    assert!(stdout.contains("implementation PR: #"), "{stdout}");
    assert!(stdout.contains("with passing CI"), "{stdout}");
    assert!(stdout.contains("actions:"), "{stdout}");
}

#[test]
fn run_fails_clearly_for_unsupported_valid_scenario() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenarios = dir.path().join("scenarios");
    write_valid_scenario(
        &scenarios,
        "alpha",
        "ready",
        "experimental",
        "Exercise delivery.",
    );

    let output = temper_scenario(&["run", &scenarios.join("alpha").to_string_lossy()]);

    assert!(!output.status.success(), "unsupported scenario should fail");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unsupported scenario `alpha`"), "{stderr}");
    assert!(stderr.contains("scenarios/basic-delivery"), "{stderr}");
}

#[test]
fn check_fails_with_human_readable_diagnostics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenario = dir.path().join("broken");
    std::fs::create_dir_all(&scenario).expect("create scenario");
    std::fs::write(
        scenario.join("scenario.toml"),
        "name = \"broken\"\n\
         status = \"unknown\"\n\
         stability = \"experimental\"\n\
         intent = { path = \"missing.md\" }\n\
         target_repo = \"not a repo\"\n\
         issue = 37\n",
    )
    .expect("write manifest");

    let output = temper_scenario(&["check", &scenario.to_string_lossy()]);

    assert!(!output.status.success(), "invalid scenario should fail");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("scenario.toml: error: status: unknown status"),
        "{stderr}"
    );
    assert!(
        stderr.contains("scenario.toml: error: intent.path: referenced path does not exist"),
        "{stderr}"
    );
    assert!(
        stderr.contains("repository must be in `owner/name` form"),
        "{stderr}"
    );
    assert!(
        stderr.contains("issue reference must include a repository"),
        "{stderr}"
    );
}

#[test]
fn default_missing_scenarios_directory_is_empty_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-scenario"))
        .arg("check")
        .current_dir(dir.path())
        .output()
        .expect("run temper-scenario");

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "OK - checked 0 scenario(s).\n"
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory has workspace crates parent")
        .parent()
        .expect("crates directory has workspace root parent")
        .to_path_buf()
}
