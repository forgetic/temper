// SPDX-License-Identifier: MPL-2.0

use crate::support::temper;

#[test]
fn top_level_help_lists_check_and_hides_internal_agent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("\n  check "), "{stdout}");
    assert!(!stdout.contains("\n  agent "), "{stdout}");
}

#[test]
fn check_help_exits_successfully() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["check", "--help"], dir.path());

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("Usage: temper [GLOBAL OPTIONS] check"),
        "{stdout}"
    );
    assert!(stdout.contains("--component"), "{stdout}");
    assert!(stdout.contains("--pool"), "{stdout}");
    assert!(stdout.contains("--strict"), "{stdout}");
}

#[test]
fn check_rejects_pool_outside_worker_component() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(
        &["check", "--component", "engine", "--pool", "builders"],
        dir.path(),
    );

    assert!(
        !output.status.success(),
        "pool without worker is a usage error"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--pool is only valid"), "{stderr}");
}
