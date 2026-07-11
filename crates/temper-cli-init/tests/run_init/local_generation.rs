// SPDX-License-Identifier: MPL-2.0

use temper_cli_common::ScriptedPrompter;
use temper_cli_init::run_init;

use super::support::{StubProvisioner, resolve_generated_bundle_non_strict};
use super::{assert_workflow_yaml, options};

#[test]
fn local_init_writes_a_complete_bundle_without_forge_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let opts = options(dir.path(), &["acme/service", "acme/docs"]);
    let mut prompt = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubProvisioner::default();

    run_init(&mut prompt, &mut provisioner, &opts).expect("init");

    assert!(provisioner.seen.is_none());
    let config = std::fs::read_to_string(dir.path().join("config.toml")).expect("config");
    assert!(
        config.contains("acme/service") && config.contains("acme/docs"),
        "{config}"
    );
    assert_workflow_yaml(&dir.path().join("workflow.yaml"));
    let credentials =
        std::fs::read_to_string(dir.path().join("credentials.toml")).expect("credentials");
    assert!(
        credentials.contains("admin-pass") && credentials.contains("sk-key"),
        "{credentials}"
    );
    assert!(!credentials.contains("admin-rest-token"), "{credentials}");
    let resolved = resolve_generated_bundle_non_strict(
        &dir.path().join("config.toml"),
        &dir.path().join("credentials.toml"),
    );
    assert_eq!(resolved.engine.repos.len(), 2);
    assert!(
        prompt.notes.iter().any(|note| note.contains("temper check")
            && note.contains("temper plan")
            && note.contains("temper apply")
            && note.contains("temper serve")),
        "{:?}",
        prompt.notes
    );
}

#[test]
fn custom_json_workflow_is_normalized_to_yaml() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("custom.json");
    std::fs::write(
        &source,
        serde_json::to_string_pretty(&super::basic_delivery_spec()).unwrap(),
    )
    .unwrap();
    let mut opts = options(dir.path(), &["acme/service"]);
    opts.overrides.workflow = Some(source.display().to_string());
    let mut prompt = ScriptedPrompter::new(Vec::<String>::new());
    run_init(&mut prompt, &mut StubProvisioner::default(), &opts).expect("init");
    assert_workflow_yaml(&dir.path().join("workflow.yaml"));
}

#[test]
fn preflight_refuses_existing_artifacts_before_apply() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.toml"), "existing").unwrap();
    let mut opts = options(dir.path(), &["acme/service"]);
    opts.apply = true;
    opts.yes = true;
    let mut provisioner = StubProvisioner::default();
    let error = run_init(
        &mut ScriptedPrompter::new(Vec::<String>::new()),
        &mut provisioner,
        &opts,
    )
    .expect_err("clobber");
    assert!(error.to_string().contains("already exist"), "{error}");
    assert!(provisioner.seen.is_none());
}
