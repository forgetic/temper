// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::support::{temper, write_valid_bundle};

#[test]
fn strict_promotes_online_note_to_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_valid_bundle(dir.path());
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
            "--strict",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "strict note should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "error");
    assert!(value["strict"].as_bool().unwrap_or(false), "{value}");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["severity"] == "note"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("online checks are not implemented"))),
        "{value}"
    );
}
