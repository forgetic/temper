// SPDX-License-Identifier: MPL-2.0

use super::support::{
    FakeForge, assert_redacted, assert_success, copy_target_fixture, rewrite_config, temper,
    temper_json,
};

#[test]
fn distributed_pool_profile_checks_and_serve_guards_are_hermetic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = copy_target_fixture("distributed-yaml", dir.path());

    let paths = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "config",
            "paths",
        ],
        dir.path(),
    );
    assert!(
        paths["workflow_file"]
            .as_str()
            .is_some_and(|path| path.ends_with("workflow.yaml")),
        "{paths}"
    );

    let offline = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "check",
            "--component",
            "worker",
            "--pool",
            "engineers",
        ],
        dir.path(),
    );
    assert_eq!(offline["status"], "ok");
    assert_eq!(offline["component"], "worker");
    assert_eq!(offline["pool"], "engineers");
    assert_eq!(offline["online"], false);

    let show = temper(
        &["--config", &bundle.to_string_lossy(), "config", "show"],
        dir.path(),
    );
    assert_success(&show);
    let show = String::from_utf8(show.stdout).expect("show stdout utf8");
    assert!(show.contains("topology     = distributed"), "{show}");
    assert!(show.contains("pools        = 2"), "{show}");
    assert!(show.contains("engineers: roles=[engineer]"), "{show}");
    assert!(show.contains("agent_profile=coding"), "{show}");
    assert!(
        show.contains("credential=coding-provider-token (available)"),
        "{show}"
    );
    assert_redacted(&show);

    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token fixture-engineer-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" => (200, r#"{"login":"engineer"}"#.to_string()),
            "/api/v1/repos/acme/service" => (200, r#"{"full_name":"acme/service"}"#.to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    rewrite_config(
        &bundle.join("config.toml"),
        "http://forge.example.invalid",
        forge.base_url(),
    );

    let online = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "check",
            "--component",
            "worker",
            "--pool",
            "engineers",
            "--online",
        ],
        dir.path(),
    );
    assert_eq!(online["status"], "ok");
    assert_eq!(online["online"], true);
    assert_redacted(&online.to_string());
    let requests = forge.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/api/v1/repos/acme/service"),
        "{requests:?}"
    );

    let missing_pool = temper(
        &["--config", &bundle.to_string_lossy(), "serve", "worker"],
        dir.path(),
    );
    assert!(!missing_pool.status.success(), "missing --pool should fail");
    let stderr = String::from_utf8(missing_pool.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("select one with `temper serve worker --pool <NAME>`"),
        "{stderr}"
    );

    let too_much_capacity = temper(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "serve",
            "worker",
            "--pool",
            "engineers",
            "--capacity",
            "3",
        ],
        dir.path(),
    );
    assert!(!too_much_capacity.status.success());
    let stderr = String::from_utf8(too_much_capacity.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--capacity 3 exceeds worker pool `engineers`"),
        "{stderr}"
    );
    assert_redacted(&stderr);
}
