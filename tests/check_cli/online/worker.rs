// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::{FakeForge, temper};

#[test]
fn worker_online_uses_role_token_without_engine_credentials() {
    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token role-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" | "/api/v1/repos/ai/temper" => (200, "{}".to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n\
             [paths]\n\
             workspace_dir = \"workspace\"\n\
             [forge]\n\
             url = \"{}\"\n\
             [[worker.pools]]\n\
             name = \"engineers\"\n\
             roles = [\"engineer\"]\n\
             repos = [\"ai/temper\"]\n\
             max_concurrent_jobs = 2\n\
             agent_profile = \"coding\"\n\
             [agent.profiles.coding]\n\
             provider = \"anthropic\"\n\
             provider_url = \"https://provider.example\"\n\
             credential = \"profile-secret\"\n",
            forge.base_url()
        ),
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [secrets]\n\
         profile-secret = \"provider-secret\"\n",
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
    assert!(
        value["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .all(|finding| !finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("engine.forge_token"))),
        "{value}"
    );
}
