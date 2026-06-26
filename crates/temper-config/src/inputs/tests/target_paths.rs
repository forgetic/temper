// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn explicit_config_directory_resolves_target_paths_under_bundle_root() {
    let dir = temp_dir("relative-target-bundle-paths");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [workflow]\n\
         file = \"flows/workflow.json\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n",
    )
    .expect("write config");

    let inputs = LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("bundle load succeeds");

    assert_eq!(
        loaded.config.as_deref(),
        Some(bundle.join("config.toml").as_path())
    );
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(bundle.join("flows/workflow.json").as_path())
    );
    assert_eq!(
        resolved.paths.state_dir.as_deref(),
        Some(bundle.join("state").as_path())
    );
    assert_eq!(resolved.worker.workspace_root, bundle.join("workspace"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explicit_config_directory_uses_target_state_for_default_workspace() {
    let dir = temp_dir("relative-target-state-default-workspace");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [paths]\n\
         state_dir = \"state\"\n",
    )
    .expect("write config");

    let inputs = LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, _loaded) = load_explicit(&inputs).expect("bundle load succeeds");

    assert_eq!(
        resolved.paths.state_dir.as_deref(),
        Some(bundle.join("state").as_path())
    );
    assert_eq!(
        resolved.worker.workspace_root,
        bundle.join("state").join("workspace")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
