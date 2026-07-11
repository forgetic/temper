// SPDX-License-Identifier: MPL-2.0

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_cli_init::{ApplyCredentialMode, ApplyOptions, run_apply};
use temper_config::{EnvMap, LoadInputs, PathResolver, load_explicit};

use super::support::{
    RecordingProvisioner, assert_redacted, assert_success, copy_target_fixture, exposed,
    rewrite_config, temper, temper_json,
};

#[test]
fn checked_in_json_and_yaml_bundles_pass_every_static_loading_seam() {
    for fixture in ["standalone-json", "distributed-yaml"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = copy_target_fixture(fixture, dir.path());
        let bundle_arg = bundle.to_string_lossy();

        let check = temper_json(
            &["--config", &bundle_arg, "--format", "json", "check"],
            dir.path(),
        );
        assert_eq!(check["status"], "ok", "{fixture}: {check}");
        assert_redacted(&check.to_string());

        let plan_json = temper(
            &["--config", &bundle_arg, "--format", "json", "plan"],
            dir.path(),
        );
        assert_success(&plan_json);
        let plan_json = String::from_utf8(plan_json.stdout).expect("plan JSON utf8");
        let report: serde_json::Value = serde_json::from_str(&plan_json).expect("plan JSON");
        assert_eq!(report["result"], "ok", "{fixture}: {report}");
        assert_eq!(report["repositories"].as_array().map(Vec::len), Some(1));
        assert!(
            report["workflow"]["path"]
                .as_str()
                .is_some_and(|path| path.starts_with(&*bundle_arg)),
            "config-relative workflow path: {report}"
        );
        assert_redacted(&plan_json);

        let plan_human = temper(&["--config", &bundle_arg, "plan"], dir.path());
        assert_success(&plan_human);
        assert_redacted(&String::from_utf8_lossy(&plan_human.stdout));

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
                credential_mode: ApplyCredentialMode::SkipLocalCredentials,
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("{fixture} applies: {error}"));
        assert_eq!(provisioner.calls.len(), 1);
        assert_eq!(provisioner.calls[0].plans.len(), 1);
        assert_redacted(&prompter.notes.join("\n"));

        // Deliberately fail after config/workflow/credentials loading and pool
        // selection. This is a bounded proof of the serve-startup loading seam,
        // not a clone of worker runtime tests.
        let (pool, capacity) = if fixture == "standalone-json" {
            ("local", "3")
        } else {
            ("engineers", "3")
        };
        let startup = temper(
            &[
                "--config",
                &bundle_arg,
                "serve",
                "worker",
                "--pool",
                pool,
                "--capacity",
                capacity,
            ],
            dir.path(),
        );
        assert!(
            !startup.status.success(),
            "startup guard must stop {fixture}"
        );
        let stderr = String::from_utf8(startup.stderr).expect("startup stderr utf8");
        assert!(
            stderr.contains("exceeds worker pool"),
            "{fixture}: {stderr}"
        );
        assert_redacted(&stderr);
    }
}

#[test]
fn config_relative_paths_and_explicit_systemd_secret_precedence_are_durable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = copy_target_fixture("standalone-json", dir.path());
    let systemd = dir.path().join("systemd-credentials");
    std::fs::create_dir_all(&systemd).expect("credential directory");
    std::fs::copy(
        bundle.join("credentials.toml"),
        systemd.join("credentials.toml"),
    )
    .expect("systemd credentials.toml");
    std::fs::write(systemd.join("engine-forge-token"), "systemd-engine-token\n")
        .expect("named systemd override");

    let mut env = EnvMap::new();
    env.insert(
        "CREDENTIALS_DIRECTORY",
        systemd.to_string_lossy().to_string(),
    );
    let loaded = load_explicit(&LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    })
    .expect("systemd-backed load");
    assert_eq!(
        exposed(loaded.0.forge.admin_token.as_ref()),
        Some("systemd-engine-token")
    );
    let workflow_path = bundle.join("workflow.json");
    assert_eq!(
        loaded.0.engine.workflow_file.as_deref(),
        Some(workflow_path.as_path())
    );
    assert_eq!(loaded.0.paths.state_dir, Some(bundle.join("state")));
    assert_eq!(loaded.0.worker.workspace_root, bundle.join("workspace"));

    let explicit_credentials = dir.path().join("explicit-credentials.toml");
    let credentials = std::fs::read_to_string(bundle.join("credentials.toml"))
        .expect("fixture credentials")
        .replace("fixture-engine-token", "explicit-engine-token");
    std::fs::write(&explicit_credentials, credentials).expect("explicit credentials");
    let explicit = load_explicit(&LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: Some(explicit_credentials.clone()),
        env: &env,
        paths: &PathResolver::default(),
    })
    .expect("explicit secret source wins");
    assert_eq!(
        exposed(explicit.0.forge.admin_token.as_ref()),
        Some("explicit-engine-token")
    );
    assert_eq!(
        explicit.1.credentials.as_deref(),
        Some(explicit_credentials.as_path())
    );

    let bundle_arg = bundle.to_string_lossy();
    let show = temper(&["--config", &bundle_arg, "config", "show"], dir.path());
    assert_success(&show);
    assert_redacted(&String::from_utf8_lossy(&show.stdout));
    assert_redacted(&String::from_utf8_lossy(&show.stderr));
    let check = temper(
        &["--config", &bundle_arg, "--format", "json", "check"],
        dir.path(),
    );
    assert_success(&check);
    assert_redacted(&String::from_utf8_lossy(&check.stdout));
    assert_redacted(&String::from_utf8_lossy(&check.stderr));
}

#[test]
fn multi_repository_plan_decline_and_yes_share_one_deployment_model() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = copy_target_fixture("standalone-json", dir.path());
    rewrite_config(
        &bundle.join("config.toml"),
        "repos = [\"acme/service\"]",
        "repos = [\"acme/service\", \"acme/docs\"]",
    );
    let bundle_arg = bundle.to_string_lossy();

    let report = temper_json(
        &["--config", &bundle_arg, "--format", "json", "plan"],
        dir.path(),
    );
    let repositories = report["repositories"].as_array().expect("repositories");
    assert_eq!(repositories.len(), 2, "{report}");
    assert_eq!(repositories[0]["repository"]["path"], "acme/service");
    assert_eq!(repositories[1]["repository"]["path"], "acme/docs");
    assert!(
        report.get("repository").is_none(),
        "no singular projection: {report}"
    );
    assert_redacted(&report.to_string());

    let credentials_before = std::fs::read(bundle.join("credentials.toml")).expect("credentials");
    let options = ApplyOptions {
        options: LoadOptions {
            config: Some(bundle.clone()),
            credentials: None,
        },
        credential_mode: ApplyCredentialMode::UpdateLocalCredentials,
        ..Default::default()
    };
    let mut decline = ScriptedPrompter::new(Vec::<String>::new());
    decline.confirmations.push_back(false);
    let mut skipped = RecordingProvisioner::default();
    run_apply(&mut decline, &mut skipped, &options).expect("decline succeeds");
    assert!(skipped.calls.is_empty(), "decline must not provision");
    assert_eq!(
        std::fs::read(bundle.join("credentials.toml")).expect("credentials after decline"),
        credentials_before,
        "decline must not mutate credentials"
    );
    assert!(
        decline
            .notes
            .iter()
            .any(|line| line.contains("Skipped forge provisioning")),
        "{:?}",
        decline.notes
    );

    let mut yes = ScriptedPrompter::new(Vec::<String>::new());
    let mut applied = RecordingProvisioner::default();
    run_apply(
        &mut yes,
        &mut applied,
        &ApplyOptions {
            yes: true,
            ..options
        },
    )
    .expect("--yes applies");
    assert_eq!(applied.calls.len(), 1, "one deployment-wide request");
    assert_eq!(applied.calls[0].plans.len(), 2);
    assert_eq!(applied.calls[0].plans[0].repo.name, "service");
    assert_eq!(applied.calls[0].plans[1].repo.name, "docs");
    assert_redacted(&decline.notes.join("\n"));
    assert_redacted(&yes.notes.join("\n"));
}
