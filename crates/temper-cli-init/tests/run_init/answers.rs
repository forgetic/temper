// SPDX-License-Identifier: MPL-2.0

use std::process::ExitCode;
use temper_cli_common::{EnvMap, LoadOptions, PathResolver, ScriptedPrompter};
use temper_cli_init::{InitOverrides, main_with_options, run_init};

use super::options;
use super::support::StubProvisioner;

#[test]
fn non_interactive_missing_admin_user_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service"]);
    opts.overrides.admin_user = None;
    let error = run_init(
        &mut ScriptedPrompter::new(Vec::<String>::new()),
        &mut StubProvisioner::default(),
        &opts,
    )
    .expect_err("missing admin");
    assert!(error.to_string().contains("--admin-user"), "{error}");
}

#[test]
fn interactive_collection_ignores_non_interactive_secret_overrides() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service"]);
    opts.non_interactive = false;
    opts.overrides = InitOverrides {
        admin_password: Some("env-password".into()),
        provider_key: Some("env-key".into()),
        ..Default::default()
    };
    let mut prompt = ScriptedPrompter::new(
        [
            "http://forge.local:3000",
            "",
            "",
            "interactive-admin",
            "interactive-password",
            "interactive-key",
        ]
        .map(str::to_string),
    );
    run_init(&mut prompt, &mut StubProvisioner::default(), &opts).expect("interactive init");
    let credentials = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(
        credentials.contains("interactive-password") && credentials.contains("interactive-key"),
        "{credentials}"
    );
    assert!(
        !credentials.contains("env-password") && !credentials.contains("env-key"),
        "{credentials}"
    );
}

#[test]
fn answers_file_drives_main_and_environment_secrets_win() {
    let dir = tempfile::tempdir().expect("tempdir");
    let answers = dir.path().join("answers.toml");
    std::fs::write(&answers, "schema_version = 1\nforge_url = \"http://forge.local:3000\"\nadmin_user = \"root\"\nadmin_password = \"answers-pw\"\nprovider = \"deepseek\"\nprovider_key = \"answers-key\"\nrepos = [\"acme/service\"]\n").unwrap();
    let mut env = EnvMap::new();
    env.insert("TEMPER_INIT_ADMIN_PASSWORD", "environment-pw");
    env.insert("TEMPER_INIT_PROVIDER_KEY", "environment-key");
    let code = main_with_options(
        vec!["--answers".into(), answers.display().to_string()],
        &env,
        &PathResolver::default(),
        LoadOptions {
            config: Some(dir.path().join("config.toml")),
            credentials: Some(dir.path().join("credentials.toml")),
        },
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let credentials = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(
        credentials.contains("environment-pw") && credentials.contains("environment-key"),
        "{credentials}"
    );
    assert!(
        !credentials.contains("answers-pw") && !credentials.contains("answers-key"),
        "{credentials}"
    );
}
