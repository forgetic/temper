// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn check(scenario: &Path, cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper-scenario-check"))
        .arg(scenario)
        .current_dir(cwd)
        .output()
        .expect("run scenario checker")
}

fn child(path: &Path, base: &str) {
    fs::create_dir_all(path).unwrap();
    fs::write(
        path.join("scenario.toml"),
        format!(
            "name = \"child\"\nintent = \"Use runtime fixtures.\"\n\
             [fixtures]\nextends = \"{base}\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn resolves_fixtures_from_the_runtime_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = workspace.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(root.join("scenarios/base")).unwrap();
    fs::write(
        root.join("scenarios/base/scenario.toml"),
        "name = \"base\"\nintent = \"Runtime fixture.\"\n\
         status = \"ready\"\nstability = \"experimental\"\n",
    )
    .unwrap();

    let local_child = root.join("scenarios/child");
    child(&local_child, "scenarios/base");
    let result = check(&local_child, outside.path());
    assert!(result.status.success(), "{result:?}");

    let external_child = outside.path().join("child");
    child(&external_child, "scenarios/base");
    let result = check(&external_child, root);
    assert!(result.status.success(), "{result:?}");
}

#[test]
fn does_not_load_fixtures_from_the_compilers_checkout() {
    let outside = tempfile::tempdir().unwrap();
    let scenario = outside.path().join("child");
    // This fixture exists in Temper's source tree, but neither the manifest
    // nor the subprocess's working directory belongs to that workspace.
    child(&scenario, "scenarios/basic-delivery");
    let result = check(&scenario, outside.path());
    assert!(!result.status.success(), "{result:?}");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("fixture inheritance base does not exist"),
        "{result:?}"
    );
}
