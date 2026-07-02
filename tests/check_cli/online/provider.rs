// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::temper;

#[test]
fn provider_profile_online_validation_redacts_secret_values() {
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
         repos = [\"ai/temper\"]\n\
         max_concurrent_jobs = 2\n\
         agent_profile = \"coding\"\n\
         [agent.profiles.coding]\n\
         provider = \"deepseek\"\n\
         provider_url = \"ftp://provider.example\"\n\
         credential = \"profile-secret\"\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [secrets]\n\
         profile-secret = \"super-secret-value\"\n",
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

    assert!(!output.status.success(), "invalid provider URL should fail");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(!stdout.contains("super-secret-value"), "{stdout}");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["check"] == "online"
            && finding["category"] == "provider"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("provider URL"))),
        "{value}"
    );
}
