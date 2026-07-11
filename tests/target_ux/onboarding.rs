// SPDX-License-Identifier: MPL-2.0

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{ApplyCredentialMode, ApplyOptions, run_apply};
use temper_config::ExposeSecret;

use super::support::{
    RecordingProvisioner, assert_finding_contains, assert_redacted, assert_success, temper,
    temper_json, temper_with_env, workspace_root,
};

#[test]
fn generated_standalone_bundle_converges_from_init_through_apply_and_runtime_adaptation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("init-bundle");
    let workflow_json =
        workspace_root().join("scenarios/target-ux-e2e/config/standalone-json/workflow.json");

    let init = temper_with_env(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "init",
            "--non-interactive",
            "--force",
            "--forge",
            "http://forge.example.invalid",
            "--repo",
            "acme/service",
            "--repo",
            "acme/docs",
            "--admin-user",
            "root",
            "--workflow",
            &workflow_json.to_string_lossy(),
            "--bind",
            "127.0.0.1:38100",
            "--provider",
            "deepseek",
            "--provider-url",
            "https://provider.example.invalid/v1",
        ],
        dir.path(),
        &[
            ("TEMPER_INIT_ADMIN_PASSWORD", "fixture-root-password"),
            ("TEMPER_INIT_PROVIDER_KEY", "fixture-provider-key"),
        ],
    );
    assert_success(&init);
    for file in [
        "config.toml",
        "credentials.toml",
        "workflow.yaml",
        "webhook-secret",
    ] {
        assert!(bundle.join(file).is_file(), "generated {file}");
    }

    let before_apply = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "check",
        ],
        dir.path(),
    );
    assert_eq!(before_apply["status"], "error");
    assert_finding_contains(
        &before_apply,
        "engine.forge_token references missing secret",
    );

    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = RecordingProvisioner::default();
    run_apply(
        &mut prompter,
        &mut provisioner,
        &ApplyOptions {
            options: LoadOptions {
                config: Some(bundle.clone()),
                credentials: Some(bundle.join("credentials.toml")),
            },
            yes: true,
            credential_mode: ApplyCredentialMode::UpdateLocalCredentials,
            ..Default::default()
        },
    )
    .expect("hermetic apply succeeds");

    assert_eq!(provisioner.calls.len(), 1, "one deployment-wide apply");
    let request = &provisioner.calls[0];
    assert_eq!(request.base_url, "http://forge.example.invalid");
    assert_eq!(request.admin_user.as_deref(), Some("root"));
    assert_eq!(
        request
            .admin_password
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("fixture-root-password")
    );
    assert_eq!(request.plans.len(), 2);
    assert_eq!(request.plans[0].repo.name, "service");
    assert_eq!(request.plans[1].repo.name, "docs");
    for plan in &request.plans {
        let webhook = plan.webhook.as_ref().expect("webhook planned");
        assert_eq!(webhook.url, "http://127.0.0.1:38100/forgejo/webhook");
    }
    assert_redacted(&prompter.notes.join("\n"));

    let credentials = std::fs::read_to_string(bundle.join("credentials.toml"))
        .expect("credentials updated by apply");
    assert!(credentials.contains("token-root"), "{credentials}");
    assert!(credentials.contains("token-engineer"), "{credentials}");

    let after_apply = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "check",
        ],
        dir.path(),
    );
    assert_eq!(after_apply["status"], "ok");
    assert_redacted(&after_apply.to_string());

    // The generated local pool/profile must reach the serve-startup runtime
    // adaptation seam. A capacity above the generated policy fails there,
    // before any network service can start.
    let startup = temper(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "serve",
            "worker",
            "--pool",
            "local",
            "--capacity",
            "3",
        ],
        dir.path(),
    );
    assert!(
        !startup.status.success(),
        "capacity guard should stop startup"
    );
    let stderr = String::from_utf8(startup.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--capacity 3 exceeds worker pool `local`"),
        "{stderr}"
    );
    assert_redacted(&stderr);
}

#[test]
fn generated_distributed_bundle_reaches_pool_and_profile_startup_adaptation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("distributed-bundle");
    let workflow =
        workspace_root().join("scenarios/target-ux-e2e/config/distributed-yaml/workflow.yaml");
    let init = temper_with_env(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "init",
            "--non-interactive",
            "--force",
            "--topology",
            "distributed",
            "--forge",
            "http://forge.example.invalid",
            "--repo",
            "acme/service",
            "--admin-user",
            "root",
            "--workflow",
            &workflow.to_string_lossy(),
            "--bind",
            "127.0.0.1:48200",
            "--provider",
            "deepseek",
            "--provider-url",
            "https://provider.example.invalid/v1",
        ],
        dir.path(),
        &[
            ("TEMPER_INIT_ADMIN_PASSWORD", "fixture-root-password"),
            ("TEMPER_INIT_PROVIDER_KEY", "fixture-provider-key"),
        ],
    );
    assert_success(&init);

    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = RecordingProvisioner::default();
    run_apply(
        &mut prompter,
        &mut provisioner,
        &ApplyOptions {
            options: LoadOptions {
                config: Some(bundle.clone()),
                credentials: Some(bundle.join("credentials.toml")),
            },
            yes: true,
            credential_mode: ApplyCredentialMode::UpdateLocalCredentials,
            ..Default::default()
        },
    )
    .expect("generated distributed apply");
    assert_eq!(provisioner.calls.len(), 1);

    let show = temper(
        &["--config", &bundle.to_string_lossy(), "config", "show"],
        dir.path(),
    );
    assert_success(&show);
    let show = String::from_utf8(show.stdout).expect("show utf8");
    assert!(show.contains("topology     = distributed"), "{show}");
    assert!(show.contains("default: roles=[engineer]"), "{show}");
    assert!(show.contains("agent_profile=default"), "{show}");
    assert_redacted(&show);

    let startup = temper(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "serve",
            "worker",
            "--pool",
            "default",
            "--capacity",
            "2",
        ],
        dir.path(),
    );
    assert!(!startup.status.success());
    let stderr = String::from_utf8(startup.stderr).expect("startup stderr utf8");
    assert!(
        stderr.contains("--capacity 2 exceeds worker pool `default`"),
        "{stderr}"
    );
    assert_redacted(&stderr);
}
