// SPDX-License-Identifier: MPL-2.0

use temper_config::{CredentialSourceKind, CredentialSourceOrigin, ExposeSecret, LoadOptions};

use super::*;

fn write_bundle(root: &std::path::Path, workflow_name: &str) {
    let workflow = temper_reference_delivery::basic_delivery_workflow_json();
    let workflow_path = root.join(workflow_name);
    if workflow_name.ends_with(".yaml") {
        let spec: temper_workflow::RawWorkflowSpec =
            serde_json::from_str(workflow).expect("workflow json");
        std::fs::write(workflow_path, serde_yaml::to_string(&spec).expect("yaml"))
            .expect("workflow");
    } else {
        std::fs::write(workflow_path, workflow).expect("workflow");
    }
    std::fs::write(
        root.join("config.toml"),
        format!(
            "schema_version = 1\n[workflow]\nfile = \"{workflow_name}\"\n[forge]\nurl = \"http://forge.local\"\nadmin = \"root\"\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [\"acme/one\", \"acme/two\"]\nroles = [\"engineer\"]\nwebhook_secret = \"hook\"\n"
        ),
    )
    .expect("config");
    std::fs::write(
        root.join("credentials.toml"),
        "schema_version = 1\n[forge.users.root]\npassword = \"admin-pass\"\n[secrets]\nhook = \"hook-secret\"\n",
    )
    .expect("credentials");
}

#[test]
fn json_and_yaml_load_to_the_same_all_repository_model() {
    let json_dir = tempfile::tempdir().expect("tempdir");
    let yaml_dir = tempfile::tempdir().expect("tempdir");
    write_bundle(json_dir.path(), "workflow.json");
    write_bundle(yaml_dir.path(), "workflow.yaml");

    let load = |root: &std::path::Path| {
        load_deployment(
            &LoadOptions {
                config: Some(root.to_path_buf()),
                credentials: None,
            },
            &temper_config::EnvMap::new(),
            &temper_config::PathResolver::default(),
            false,
        )
        .expect("deployment")
    };
    let json = load(json_dir.path());
    let yaml = load(yaml_dir.path());

    assert_eq!(json.workflow.name(), yaml.workflow.name());
    assert_eq!(json.repositories.len(), 2);
    assert_eq!(yaml.repositories.len(), 2);
    for desired in json.repositories.iter().chain(&yaml.repositories) {
        assert!(!desired.plan.repository_auto_init);
        assert!(desired.plan.seed_commits.is_empty());
        assert!(
            desired.plan.webhook.is_none(),
            "secret is attached at call boundary"
        );
        assert!(!desired.plan.labels.is_empty());
    }
    assert_eq!(
        json.credential_source.as_ref().map(|source| source.origin),
        Some(CredentialSourceOrigin::ConfigSibling)
    );
    assert_eq!(
        json.credential_source.as_ref().map(|source| source.kind),
        Some(CredentialSourceKind::File)
    );
    assert_eq!(
        json.webhook
            .as_ref()
            .map(|webhook| webhook.secret.expose_secret()),
        Some("hook-secret")
    );
}
