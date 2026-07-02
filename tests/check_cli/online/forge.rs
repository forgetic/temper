// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::{FakeForge, temper, write_online_engine_bundle};

#[test]
fn online_forge_success_checks_user_and_repos() {
    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token forge-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" => (200, r#"{"login":"engineer"}"#.to_string()),
            "/api/v1/repos/ai/temper" => (200, r#"{"full_name":"ai/temper"}"#.to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_online_engine_bundle(dir.path(), forge.base_url());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
        ],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["online"], true);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("online checks are not implemented"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let paths = forge
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&"/api/v1/user".to_string()), "{paths:?}");
    assert!(
        paths.contains(&"/api/v1/repos/ai/temper".to_string()),
        "{paths:?}"
    );
}

#[test]
fn online_forge_auth_failure_is_distinct_and_redacted() {
    let forge = FakeForge::start(|request| match request.path.as_str() {
        "/api/v1/user" => (401, "{}".to_string()),
        _ => (404, "{}".to_string()),
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_online_engine_bundle(dir.path(), forge.base_url());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "auth failure should fail");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(!stdout.contains("forge-token"), "{stdout}");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["check"] == "online"
            && finding["category"] == "auth"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("authentication failed"))),
        "{value}"
    );
}

#[test]
fn online_forge_reports_missing_repo_visibility() {
    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token forge-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" => (200, "{}".to_string()),
            "/api/v1/repos/ai/temper" => (404, "{}".to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_online_engine_bundle(dir.path(), forge.base_url());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "missing repo should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["check"] == "online"
            && finding["category"] == "repo"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("ai/temper"))),
        "{value}"
    );
}
