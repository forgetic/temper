// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::temper;

#[test]
fn strict_promotes_online_note_to_failure() {
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
         capabilities = [\"ai/temper:engineer\"]\n\
         [agent]\n\
         provider = \"chatgpt\"\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [agent.providers.chatgpt]\n\
         type = \"oauth\"\n\
         access = \"access-token\"\n\
         refresh = \"refresh-token\"\n\
         expires = 0\n",
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
            "--online",
            "--strict",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "strict online note should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "error");
    assert!(value["strict"].as_bool().unwrap_or(false), "{value}");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "note"
            && finding["check"] == "online"
            && finding["category"] == "provider"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("refresh token is configured"))),
        "{value}"
    );
}
