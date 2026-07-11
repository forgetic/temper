// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use temper_cli_init::{ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner};
use temper_config::Secret;
use temper_forge::RepositoryId;
use temper_provision::{Provisioned, RoleIdentity};

pub(crate) use crate::check_support::FakeForge;

#[derive(Default)]
pub(crate) struct RecordingProvisioner {
    pub(crate) calls: Vec<ApplyPlanRequest>,
}

impl ApplyProvisioner for RecordingProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String> {
        self.calls.push(request.clone());
        let identity = |user: &str| RoleIdentity {
            user: user.to_string(),
            email: format!("{user}@example.invalid"),
            token: format!("token-{user}"),
            password: format!("pw-{user}"),
        };
        let provisioned = request
            .plans
            .iter()
            .map(|plan| {
                let roles = plan
                    .roles
                    .iter()
                    .map(|binding| (binding.role.clone(), identity(&binding.user.handle)))
                    .collect::<BTreeMap<_, _>>();
                Provisioned {
                    owner: plan.repo.owner.clone(),
                    name: plan.repo.name.clone(),
                    repository: RepositoryId::new(format!(
                        "{}/{}",
                        plan.repo.owner, plan.repo.name
                    )),
                    roles,
                    automation: identity(&plan.automation_login),
                }
            })
            .collect();
        Ok(ApplyPlanOutcome {
            provisioned,
            admin_token: Secret::from("token-root"),
        })
    }
}

pub(crate) fn temper(args: &[&str], env_root: &Path) -> Output {
    temper_with_env(args, env_root, &[])
}

pub(crate) fn temper_with_env(args: &[&str], env_root: &Path, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_temper"));
    command
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .env_remove("CREDENTIALS_DIRECTORY");
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("run temper")
}

pub(crate) fn temper_json(args: &[&str], env_root: &Path) -> Value {
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

pub(crate) fn copy_target_fixture(name: &str, root: &Path) -> PathBuf {
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

pub(crate) fn rewrite_config(config_path: &Path, from: &str, to: &str) {
    let config = std::fs::read_to_string(config_path).expect("read config");
    assert!(config.contains(from), "missing `{from}` in {config}");
    std::fs::write(config_path, config.replace(from, to)).expect("rewrite config");
}

pub(crate) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn assert_finding_contains(report: &Value, needle: &str) {
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["message"]
            .as_str()
            .is_some_and(|message| message.contains(needle))),
        "{report}"
    );
}

pub(crate) fn assert_redacted(rendered: &str) {
    for secret in [
        "fixture-root-password",
        "fixture-root-token",
        "fixture-bot-password",
        "fixture-bot-token",
        "fixture-architect-token",
        "fixture-engineer-token",
        "fixture-reviewer-token",
        "fixture-engine-token",
        "fixture-webhook-secret",
        "fixture-worker-token",
        "fixture-worker-engineers-token",
        "fixture-worker-reviewers-token",
        "fixture-provider-token",
        "fixture-coding-provider-token",
        "fixture-review-provider-token",
        "fixture-deepseek-key",
        "explicit-engine-token",
        "systemd-engine-token",
    ] {
        assert!(
            !rendered.contains(secret),
            "secret `{secret}` leaked: {rendered}"
        );
    }
}

pub(crate) fn exposed(secret: Option<&Secret>) -> Option<&str> {
    use temper_config::ExposeSecret;
    secret.map(ExposeSecret::expose_secret)
}

pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
