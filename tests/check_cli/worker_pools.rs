// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::temper;

#[test]
fn check_json_fails_for_target_pool_profile_validation_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         max_concurrent_jobs = 0\n",
    )
    .expect("write config");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "invalid pool should fail check");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("worker.pools[0].max_concurrent_jobs"))),
        "{value}"
    );
}

#[test]
fn check_json_fails_for_target_pool_missing_capacity_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [paths]\n\
         workspace_dir = \"workspace\"\n\
         [worker]\n\
         git_base_url = \"https://git.example\"\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
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

    assert!(
        !output.status.success(),
        "missing capacity should fail check"
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("max_concurrent_jobs"))),
        "{value}"
    );
}

#[test]
fn worker_pool_scope_ignores_unrelated_engine_secret() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("state")).expect("create state");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         [engine]\n\
         forge_token = \"missing-engine-token\"\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"architect\"]\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n\
         max_concurrent_jobs = 2\n\
         agent_profile = \"coding\"\n\
         [agent.profiles.coding]\n\
         credential = \"profile-secret\"\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [secrets]\n\
         profile-secret = \"provider-key\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
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

    assert!(
        output.status.success(),
        "worker scope should ignore engine-only secret: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["component"], "worker");
    assert_eq!(value["pool"], "engineers");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().all(|finding| !finding["message"]
            .as_str()
            .is_some_and(|message| message.contains("engine.forge_token"))),
        "{value}"
    );
}
