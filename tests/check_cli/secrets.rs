// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::temper;

#[test]
fn check_json_fails_for_missing_named_secret_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         [engine]\n\
         forge_token = \"missing-engine-token\"\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    std::fs::write(bundle.join("credentials.toml"), "schema_version = 1\n")
        .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );

    assert!(!output.status.success(), "missing secret should fail check");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "error"
            && finding["message"].as_str().is_some_and(|message| {
                message.contains("engine.forge_token") && message.contains("missing-engine-token")
            })),
        "{value}"
    );
}
