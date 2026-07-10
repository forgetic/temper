// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .env_remove("CREDENTIALS_DIRECTORY")
        .output()
        .expect("run temper")
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout utf8")
}

#[test]
fn top_level_help_exposes_only_the_public_operator_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let help = stdout(temper(&["--help"], dir.path()));

    let mut previous = 0;
    for command in ["init", "plan", "apply", "check", "serve", "config"] {
        let marker = format!("\n  {command} ");
        let position = help
            .find(&marker)
            .unwrap_or_else(|| panic!("missing public command `{command}`: {help}"));
        assert!(position >= previous, "public commands out of order: {help}");
        previous = position;
    }
    for hidden in ["daemon", "agent", "trigger-forgejo"] {
        assert!(
            !help.contains(&format!("\n  {hidden} ")),
            "hidden command `{hidden}` leaked into top-level help: {help}"
        );
    }
}

#[test]
fn hidden_compatibility_and_internal_commands_remain_dispatchable() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (args, expected) in [
        (&["daemon", "--help"][..], "Legacy compatibility command"),
        (&["agent", "--help"][..], "temper agent --context"),
        (&["trigger-forgejo", "--help"][..], "temper-trigger-forgejo"),
    ] {
        let help = stdout(temper(args, dir.path()));
        assert!(help.contains(expected), "args={args:?}: {help}");
    }

    let config_help = stdout(temper(&["config", "--help"], dir.path()));
    assert!(
        config_help.contains("Compatibility commands:"),
        "{config_help}"
    );
    assert!(config_help.contains("\n  validate  "), "{config_help}");
    assert!(config_help.contains("\n  init      "), "{config_help}");

    let bundle = dir.path().join("compat-bundle");
    let bundle_arg = bundle.to_string_lossy();
    let initialized = stdout(temper(
        &["--config", &bundle_arg, "config", "init"],
        dir.path(),
    ));
    assert!(initialized.contains("Wrote"), "{initialized}");

    // Keep the generated compatibility template but narrow its legacy
    // cross-product to the identity the template credentials already contain.
    let config_path = bundle.join("config.toml");
    let config = std::fs::read_to_string(&config_path).expect("read generated config");
    std::fs::write(
        &config_path,
        config.replace(
            "roles = [\"architect\", \"engineer\", \"code-reviewer\"]",
            "roles = [\"engineer\"]",
        ),
    )
    .expect("update generated config");
    let validated = stdout(temper(
        &["--config", &bundle_arg, "config", "validate"],
        dir.path(),
    ));
    assert!(validated.contains("config:"), "{validated}");
}

#[test]
fn existing_repo_help_labels_supported_compatibility_behavior() {
    let dir = tempfile::tempdir().expect("tempdir");
    for command in ["init", "plan", "apply"] {
        let help = stdout(temper(&[command, "--help"], dir.path()));
        assert!(help.contains("--existing-repo"), "{command}: {help}");
        assert!(
            help.contains("Supported compatibility behavior"),
            "{command}: {help}"
        );
    }
}
