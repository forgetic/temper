// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::{Command, Output};

fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .output()
        .expect("run temper")
}

#[test]
fn config_show_includes_target_pools_and_agent_profiles_without_secret_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [deployment]\n\
         standalone_shutdown_budget_secs = 40\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"agent\"\n\
         [engine]\n\
         forge_token = \"forge-engine-token\"\n\
         webhook_secret = \"webhook-secret\"\n\
         ci_missing_grace_secs = 41\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n\
         [worker]\n\
         max_no_progress_secs = 777\n\
         max_run_secs = 3600\n\
         graceful_cancellation_grace_secs = 12\n\
         forced_termination_grace_secs = 6\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n\
         max_concurrent_jobs = 2\n\
         agent_profile = \"coding\"\n\
         worker_token = \"worker-engineers-token\"\n\
         [agent]\n\
         provider = \"anthropic\"\n\
         [agent.deadlines]\n\
         tool_timeout_secs = 500\n\
         model_connect_timeout_secs = 100\n\
         model_idle_timeout_secs = 90\n\
         [agent.profiles.coding]\n\
         command = [\"temper\", \"agent\"]\n\
         provider = \"anthropic\"\n\
         model = \"claude-opus-4-8\"\n\
         investigate_model = \"claude-haiku-4-5\"\n\
         provider_url = \"http://fake-llm\"\n\
         max_iterations = 250\n\
         subagents = true\n\
         credential = \"agent-provider\"\n\
         [agent.profiles.coding.deadlines]\n\
         model_idle_timeout_secs = 45\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.agent]\n\
         token = \"super-secret-forge-token\"\n\
         [secrets]\n\
         forge-engine-token = \"super-secret-named-forge-token\"\n\
         webhook-secret = \"super-secret-webhook-value\"\n\
         worker-engineers-token = \"super-secret-worker-token\"\n\
         agent-provider = \"super-secret-agent-provider\"\n\
         [agent.providers.anthropic]\n\
         type = \"api-key\"\n\
         key = \"super-secret-provider-key\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(&["--config", &bundle_arg, "config", "show"], dir.path());

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    assert!(
        stdout.contains("forge_token  = forge-engine-token (available)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("webhook_secret = webhook-secret (available)"),
        "{stdout}"
    );
    assert!(stdout.contains("ci_missing_grace = 41s"), "{stdout}");
    assert!(stdout.contains("pools        = 1"), "{stdout}");
    assert!(
        stdout.contains("engineers: roles=[engineer], repos=[ai/temper]"),
        "{stdout}"
    );
    assert!(stdout.contains("agent_profile=coding"), "{stdout}");
    assert!(
        stdout.contains("worker_token=worker-engineers-token (available)"),
        "{stdout}"
    );
    assert!(stdout.contains("profiles     = 1"), "{stdout}");
    assert!(stdout.contains("subagents    = false"), "{stdout}");
    assert!(
        stdout.contains("coding: command=[temper agent]"),
        "{stdout}"
    );
    assert!(stdout.contains("provider=anthropic"), "{stdout}");
    assert!(stdout.contains("model=claude-opus-4-8"), "{stdout}");
    assert!(
        stdout.contains("credential=agent-provider (available)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("standalone_shutdown_budget_secs = 40"),
        "{stdout}"
    );
    assert!(stdout.contains("max_no_progress_secs = 777"), "{stdout}");
    assert!(stdout.contains("max_run_secs = 3600"), "{stdout}");
    assert!(
        stdout.contains("cancellation_graces_secs = 12 graceful + 6 forced"),
        "{stdout}"
    );
    assert!(
        stdout.contains("deadlines_secs = tool 500, model_connect 100, model_idle 90"),
        "{stdout}"
    );
    assert!(
        stdout.contains("deadlines_secs=tool 500/connect 100/idle 45"),
        "{stdout}"
    );
    assert!(!stdout.contains("super-secret-forge-token"), "{stdout}");
    assert!(
        !stdout.contains("super-secret-named-forge-token"),
        "{stdout}"
    );
    assert!(!stdout.contains("super-secret-webhook-value"), "{stdout}");
    assert!(!stdout.contains("super-secret-worker-token"), "{stdout}");
    assert!(!stdout.contains("super-secret-agent-provider"), "{stdout}");
    assert!(!stdout.contains("super-secret-provider-key"), "{stdout}");
}
