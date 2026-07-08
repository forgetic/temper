// SPDX-License-Identifier: MPL-2.0

#[allow(dead_code)]
#[path = "check_cli/support.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{ApplyOptions, ProvisionOutcome, ProvisionRequest, Provisioner, run_apply};
use temper_forge::RepositoryId;
use temper_provision::{Provisioned, RoleIdentity};
use temper_workflow::RoleId;

use support::{FakeForge, temper};

#[derive(Default)]
struct RecordingProvisioner {
    seen: Option<ProvisionRequest>,
}

impl Provisioner for RecordingProvisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String> {
        self.seen = Some(request.clone());
        let identity = |user: &str| RoleIdentity {
            user: user.to_string(),
            email: format!("{user}@example.invalid"),
            token: format!("token-{user}"),
            password: format!("pw-{user}"),
        };
        let mut roles = BTreeMap::new();
        roles.insert(RoleId::new("architect"), identity("architect"));
        roles.insert(RoleId::new("engineer"), identity("engineer"));
        roles.insert(RoleId::new("reviewer"), identity("reviewer"));
        Ok(ProvisionOutcome {
            provisioned: Provisioned {
                owner: request.owner.clone(),
                name: request.name.clone(),
                repository: RepositoryId::new(format!("{}/{}", request.owner, request.name)),
                roles,
                automation: identity("bot"),
            },
            admin_token: "token-root".to_string(),
        })
    }
}

#[test]
fn target_ux_init_check_apply_flow_uses_json_input_and_yaml_bundle() {
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
    assert!(bundle.join("config.toml").is_file());
    assert!(bundle.join("credentials.toml").is_file());
    assert!(bundle.join("workflow.yaml").is_file());
    assert!(bundle.join("webhook-secret").is_file());

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
    assert_finding_contains(&before_apply, "forge admin token is unset");

    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = RecordingProvisioner::default();
    run_apply(
        &mut prompter,
        &mut provisioner,
        &ApplyOptions {
            options: LoadOptions {
                config: Some(bundle.clone()),
                credentials: None,
            },
            yes: true,
            ..Default::default()
        },
    )
    .expect("hermetic apply succeeds");

    let request = provisioner.seen.expect("apply planned provisioning");
    assert_eq!(request.base_url, "http://forge.example.invalid");
    assert_eq!(request.owner, "acme");
    assert_eq!(request.name, "service");
    assert_eq!(
        request.webhook_url,
        "http://127.0.0.1:38100/forgejo/webhook"
    );
    assert_eq!(
        request.workflow_path.as_deref(),
        Some(bundle.join("workflow.yaml").as_path())
    );

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
    let credentials = std::fs::read_to_string(bundle.join("credentials.toml"))
        .expect("credentials updated by apply");
    assert!(credentials.contains("token-root"), "{credentials}");
    assert!(credentials.contains("token-engineer"), "{credentials}");

    let help = temper(&["serve", "standalone", "--help"], dir.path());
    assert_success(&help);
    let stdout = String::from_utf8(help.stdout).expect("stdout utf8");
    assert!(stdout.contains("serve standalone"), "{stdout}");
    assert!(stdout.contains("--id <ID>"), "{stdout}");

    let rejected = temper(&["serve", "standalone", "--pool", "engineers"], dir.path());
    assert!(!rejected.status.success(), "worker-only flag should fail");
    let stderr = String::from_utf8(rejected.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("cannot be used with `temper serve standalone`"),
        "{stderr}"
    );
}

#[test]
fn target_ux_distributed_worker_pool_checks_and_serve_guards_are_hermetic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = copy_target_fixture("distributed-yaml", dir.path());

    let paths = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "config",
            "paths",
        ],
        dir.path(),
    );
    assert!(
        paths["workflow_file"]
            .as_str()
            .is_some_and(|path| path.ends_with("workflow.yaml")),
        "{paths}"
    );

    let offline = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "--format",
            "json",
            "check",
            "--component",
            "worker",
            "--pool",
            "engineers",
        ],
        dir.path(),
    );
    assert_eq!(offline["status"], "ok");
    assert_eq!(offline["component"], "worker");
    assert_eq!(offline["pool"], "engineers");
    assert_eq!(offline["online"], false);

    let show = temper(
        &["--config", &bundle.to_string_lossy(), "config", "show"],
        dir.path(),
    );
    assert_success(&show);
    let show = String::from_utf8(show.stdout).expect("show stdout utf8");
    assert!(show.contains("topology     = distributed"), "{show}");
    assert!(show.contains("pools        = 2"), "{show}");
    assert!(show.contains("engineers: roles=[engineer]"), "{show}");
    assert!(show.contains("agent_profile=coding"), "{show}");
    assert!(
        show.contains("credential=coding-provider-token (available)"),
        "{show}"
    );
    assert!(!show.contains("fixture-engineer-token"), "{show}");
    assert!(!show.contains("fixture-coding-provider-token"), "{show}");

    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token fixture-engineer-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" => (200, r#"{"login":"engineer"}"#.to_string()),
            "/api/v1/repos/acme/service" => (200, r#"{"full_name":"acme/service"}"#.to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    rewrite_forge_url(&bundle.join("config.toml"), forge.base_url());

    let online = temper_json(
        &[
            "--config",
            &bundle.to_string_lossy(),
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
    assert_eq!(online["status"], "ok");
    assert_eq!(online["online"], true);
    let requests = forge.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/api/v1/repos/acme/service"),
        "{requests:?}"
    );

    let missing_pool = temper(
        &["--config", &bundle.to_string_lossy(), "serve", "worker"],
        dir.path(),
    );
    assert!(
        !missing_pool.status.success(),
        "missing --pool should fail before startup"
    );
    let stderr = String::from_utf8(missing_pool.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("select one with `temper serve worker --pool <NAME>`"),
        "{stderr}"
    );

    let too_much_capacity = temper(
        &[
            "--config",
            &bundle.to_string_lossy(),
            "serve",
            "worker",
            "--pool",
            "engineers",
            "--capacity",
            "3",
        ],
        dir.path(),
    );
    assert!(
        !too_much_capacity.status.success(),
        "capacity above pool policy should fail before startup"
    );
    let stderr = String::from_utf8(too_much_capacity.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--capacity 3 exceeds worker pool `engineers`"),
        "{stderr}"
    );
}

#[test]
fn target_ux_trigger_contract_is_selected_and_documented() {
    let dir = tempfile::tempdir().expect("tempdir");

    let selected = temper(&["trigger-forgejo", "--help"], dir.path());
    assert_success(&selected);
    let stdout = String::from_utf8(selected.stdout).expect("stdout utf8");
    assert!(stdout.contains("temper-trigger-forgejo"), "{stdout}");
    assert!(stdout.contains("--webhook-secret-file"), "{stdout}");
    assert!(stdout.contains("--wake-dir"), "{stdout}");

    let rejected = temper(&["serve", "trigger"], dir.path());
    assert!(
        !rejected.status.success(),
        "serve trigger is intentionally rejected"
    );
    let stderr = String::from_utf8(rejected.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("`temper serve trigger` is not implemented yet"),
        "{stderr}"
    );
}

fn temper_with_env(args: &[&str], env_root: &Path, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper"));
    command
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"));
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("run temper")
}

fn temper_json(args: &[&str], env_root: &Path) -> Value {
    let output = temper(args, env_root);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    if !output.status.success() && stdout.trim().is_empty() {
        panic!(
            "command failed without JSON stdout; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("valid JSON ({error}): {stdout}"))
}

fn copy_target_fixture(name: &str, root: &Path) -> PathBuf {
    let source = workspace_root()
        .join("scenarios/target-ux-e2e/config")
        .join(name);
    let destination = root.join(name);
    copy_dir(&source, &destination);
    std::fs::create_dir_all(destination.join("state")).expect("state dir");
    std::fs::create_dir_all(destination.join("workspace")).expect("workspace dir");
    destination
}

fn copy_dir(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).expect("create destination dir");
    for entry in std::fs::read_dir(source).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir(&source_path, &destination_path);
        } else {
            std::fs::copy(&source_path, &destination_path).unwrap_or_else(|error| {
                panic!(
                    "copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            });
        }
    }
}

fn rewrite_forge_url(config_path: &Path, forge_url: &str) {
    let config = std::fs::read_to_string(config_path).expect("read config");
    assert!(config.contains("http://forge.example.invalid"), "{config}");
    std::fs::write(
        config_path,
        config.replace("http://forge.example.invalid", forge_url),
    )
    .expect("rewrite forge URL");
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_finding_contains(report: &Value, needle: &str) {
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["message"]
            .as_str()
            .is_some_and(|message| message.contains(needle))),
        "{report}"
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
