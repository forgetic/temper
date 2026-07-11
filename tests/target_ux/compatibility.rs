// SPDX-License-Identifier: MPL-2.0

use super::support::{assert_success, temper, workspace_root};

#[test]
fn public_help_promotes_complete_flow_while_internal_commands_remain_hidden() {
    let dir = tempfile::tempdir().expect("tempdir");
    let top = temper(&["--help"], dir.path());
    assert_success(&top);
    let top = String::from_utf8(top.stdout).expect("top-level help utf8");
    let mut prior = 0;
    for command in ["init", "plan", "apply", "check", "serve", "config"] {
        let marker = format!("\n  {command} ");
        let position = top
            .find(&marker)
            .unwrap_or_else(|| panic!("missing public command `{command}`: {top}"));
        assert!(position >= prior, "public command order: {top}");
        prior = position;
    }
    for hidden in ["daemon", "agent", "trigger-forgejo"] {
        assert!(!top.contains(&format!("\n  {hidden} ")), "{top}");
    }

    for (args, expected) in [
        (&["daemon", "--help"][..], "Legacy compatibility command"),
        (&["agent", "--help"][..], "temper agent --context"),
        (&["trigger-forgejo", "--help"][..], "temper-trigger-forgejo"),
    ] {
        let output = temper(args, dir.path());
        assert_success(&output);
        let stdout = String::from_utf8(output.stdout).expect("hidden help utf8");
        assert!(stdout.contains(expected), "args={args:?}: {stdout}");
    }

    for command in ["init", "plan", "apply"] {
        let output = temper(&[command, "--help"], dir.path());
        assert_success(&output);
        let stdout = String::from_utf8(output.stdout).expect("subcommand help utf8");
        assert!(stdout.contains("--existing-repo"), "{command}: {stdout}");
        assert!(
            stdout.contains("Supported compatibility behavior"),
            "{command}: {stdout}"
        );
    }
}

#[test]
fn audit_matrix_cites_focused_compatibility_and_webhook_authorities() {
    let root = workspace_root();
    let matrix =
        std::fs::read_to_string(root.join("docs/reference/target-era-operator-contract-matrix.md"))
            .expect("audit matrix");
    for group in [
        "CLI",
        "Loading and secrets",
        "Workflows",
        "Init",
        "Plan and apply",
        "Workers and agents",
        "Webhook",
        "Docs and systemd",
        "Target-UX scenario",
    ] {
        assert!(
            matrix.contains(&format!("| {group} |")),
            "missing {group}: {matrix}"
        );
    }
    for focused_test in [
        "no_pool_registration_preserves_legacy_capabilities_even_with_policies",
        "register_without_pool_preserves_legacy_capabilities_with_pool_policies",
        "pool_without_agent_profile_uses_legacy_provider_fallback",
        "legacy_only_config_has_no_target_metadata_and_preserves_runtime_fields",
        "posted_webhook_wakes_target_then_worker_is_assigned",
        "posted_webhook_drives_success_apply_to_pull_request",
        "selected_forgejo_contract_accepts_forgejo_headers_and_sha256_prefix",
    ] {
        assert!(
            matrix.contains(focused_test),
            "matrix must cite {focused_test}"
        );
    }
    for heading in [
        "### Confirmed gaps",
        "### Resolutions",
        "### Deliberately retained compatibility surfaces",
        "### Unchanged areas",
    ] {
        assert!(matrix.contains(heading), "matrix lacks {heading}");
    }

    let reference_index =
        std::fs::read_to_string(root.join("docs/reference/README.md")).expect("reference index");
    assert!(
        reference_index.contains("target-era-operator-contract-matrix.md"),
        "matrix is not linked from reference index"
    );
}
