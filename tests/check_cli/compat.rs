// SPDX-License-Identifier: MPL-2.0

use crate::support::temper;

#[test]
fn config_validate_remains_dispatchable_for_compatibility() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = temper(&["config", "validate"], dir.path());

    assert!(
        !output.status.success(),
        "compatibility path should preserve strict validation status"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("config:      (none"), "{stdout}");
    assert!(stdout.contains("error: forge URL is unset"), "{stdout}");
}
