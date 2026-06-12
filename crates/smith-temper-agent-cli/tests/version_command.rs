use std::process::Command;

#[test]
fn version_prints_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_smith-temper-agent-cli"))
        .arg("version")
        .output()
        .expect("smith-temper-agent-cli runs");

    assert!(
        output.status.success(),
        "version command should exit 0; stderr was {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = format!("smith-temper-agent-cli {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(output.stdout, expected.as_bytes());
    assert!(
        output.stderr.is_empty(),
        "version command should not write stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn help_lists_version_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_smith-temper-agent-cli"))
        .arg("help")
        .output()
        .expect("smith-temper-agent-cli runs");

    assert!(
        output.status.success(),
        "help command should exit 0; stderr was {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("help stdout is utf-8");
    assert!(
        stdout.contains("smith-temper-agent-cli version"),
        "help output should list version command: {stdout}"
    );
}
