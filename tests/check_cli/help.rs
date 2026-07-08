// SPDX-License-Identifier: MPL-2.0

use crate::support::temper;

const COMPONENT_CHOICES: &str = "standalone|engine|worker";

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
    assert!(stdout.contains(COMPONENT_CHOICES), "{stdout}");
    assert!(
        !stdout.contains(&format!("{COMPONENT_CHOICES}|trigger")),
        "{stdout}"
    );
    assert!(!stdout.contains("--component <trigger"), "{stdout}");
    assert!(
        stdout.contains("webhook intake is validated under engine or standalone"),
        "{stdout}"
    );
    assert!(stdout.contains("--pool"), "{stdout}");
    assert!(stdout.contains("--strict"), "{stdout}");
}

#[test]
fn check_rejects_trigger_component_with_guidance() {
    for args in [
        vec!["check", "--component", "trigger"],
        vec!["check", "--component=trigger"],
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = temper(&args, dir.path());

        assert!(
            !output.status.success(),
            "trigger component is a usage error"
        );
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            stderr.contains("unsupported --component `trigger`"),
            "{stderr}"
        );
        assert!(stderr.contains("webhook intake is validated"), "{stderr}");
        assert!(stderr.contains("--component engine"), "{stderr}");
        assert!(stderr.contains("--component standalone"), "{stderr}");
        assert!(stderr.contains(COMPONENT_CHOICES), "{stderr}");
        let legacy_choices = format!("{COMPONENT_CHOICES}|trigger");
        for forbidden in [
            "trigger component checks",
            "not implemented yet",
            legacy_choices.as_str(),
        ] {
            assert!(!stderr.contains(forbidden), "{stderr}");
        }
    }
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
