// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use temper_benchmark_cli::{
    BASELINE_SNAPSHOT_VERSION, BENCHMARK_MANIFEST_SCHEMA, BenchmarkArtifactLayout,
    BenchmarkManifestError, load_benchmark_manifest, prepare_benchmark_workspace,
};
use tempfile::TempDir;

fn write_context(root: &Path, repositories: &[(&str, &str)]) {
    let repos = repositories
        .iter()
        .map(|(id, dir)| {
            json!({
                "id": id,
                "owner": "acme",
                "name": id,
                "default_branch": "main",
                "dir": dir,
                "access": "writable",
                "base_branch": "main",
                "branch_hint": format!("benchmark/{id}")
            })
        })
        .collect::<Vec<_>>();
    let context = json!({
        "repos": repos,
        "work_item": {
            "role": "engineer",
            "queue": "code_ready",
            "kind": "code",
            "target": "Issue { number: ItemNumber(1) }",
            "context": "{}"
        },
        "action": "open_pr",
        "correlation_key": "benchmark-fixture",
        "checkout": "writable"
    });
    fs::write(
        root.join("context.json"),
        serde_json::to_vec_pretty(&context).unwrap(),
    )
    .unwrap();
}

fn write_manifest(root: &Path, fixture: &str) -> PathBuf {
    let manifest = root.join("benchmark.toml");
    fs::write(
        &manifest,
        format!(
            r#"schema = "{BENCHMARK_MANIFEST_SCHEMA}"
name = "secure-workspace"
fixture = {fixture}
workspace_context = "context.json"
capture = "diagnostic"
validation_command_prefixes = [["cargo", "test"], ["./.temper/pre-pr"]]
discovery_command_prefixes = [["git", "grep"]]
post_run_commands = [["cargo", "test", "-p", "fixture"]]
jig_script = "jig.json"
repetitions = 2

[[graph_decision_targets]]
target = "one/src/lib.rs"
kind = "implementation"

[[graph_decision_targets.consumption]]
tool = "search_code"
target = "worker_slot"

[annotations]
provider_region = "local"
cache_warmth = "cold"
"#
        ),
    )
    .unwrap();
    manifest
}

fn benchmark(repositories: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let fixture = root.path().join("fixture");
    fs::create_dir(&fixture).unwrap();
    for (id, dir) in repositories {
        let repository = fixture.join(dir);
        fs::create_dir_all(&repository).unwrap();
        fs::write(repository.join("README.md"), format!("# {id}\n")).unwrap();
    }
    write_context(root.path(), repositories);
    fs::write(root.path().join("jig.json"), b"{}\n").unwrap();
    let manifest = write_manifest(root.path(), "\"fixture\"");
    (root, manifest)
}

#[test]
fn manifest_resolves_inputs_relative_to_its_own_directory() {
    let (_root, manifest_path) = benchmark(&[("one", "one")]);
    let manifest = load_benchmark_manifest(&manifest_path).unwrap();

    assert_eq!(manifest.manifest().name, "secure-workspace");
    assert_eq!(manifest.manifest().repetitions, 2);
    assert_eq!(
        manifest.manifest().discovery_command_prefixes[0],
        ["git", "grep"]
    );
    assert_eq!(
        manifest.manifest().graph_decision_targets[0].target,
        "one/src/lib.rs"
    );
    assert_eq!(
        manifest.manifest().graph_decision_targets[0].consumption[0].target,
        "worker_slot"
    );
    assert_eq!(
        manifest.manifest().annotations.provider_region.as_deref(),
        Some("local")
    );
    assert_eq!(manifest.fixture_dir().file_name().unwrap(), "fixture");
    assert_eq!(
        manifest.workspace_context_path().file_name().unwrap(),
        "context.json"
    );
    assert_eq!(manifest.jig_script_path().file_name().unwrap(), "jig.json");
}

#[test]
fn manifest_rejects_non_targeted_graph_consumers() {
    let (root, manifest_path) = benchmark(&[("one", "one")]);
    let source = fs::read_to_string(&manifest_path).unwrap();
    fs::write(
        &manifest_path,
        source.replace("tool = \"search_code\"", "tool = \"read\""),
    )
    .unwrap();

    let error = load_benchmark_manifest(manifest_path).unwrap_err();
    assert!(error.to_string().contains("must be a targeted graph tool"));
    drop(root);
}

#[test]
fn manifest_rejects_absolute_parent_and_missing_paths() {
    let attacks = [
        ("\"../fixture\"".to_string(), "parent traversal"),
        ("\"missing\"".to_string(), "input does not exist"),
    ];
    for (fixture, expected) in attacks {
        let (root, _) = benchmark(&[("one", "one")]);
        let manifest_path = write_manifest(root.path(), &fixture);
        let error = load_benchmark_manifest(manifest_path).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
    }

    let (root, _) = benchmark(&[("one", "one")]);
    let absolute = serde_json::to_string(&root.path().join("fixture").to_string_lossy()).unwrap();
    let manifest_path = write_manifest(root.path(), &absolute);
    let error = load_benchmark_manifest(manifest_path).unwrap_err();
    assert!(error.to_string().contains("absolute paths are not allowed"));

    let (root, manifest_path) = benchmark(&[("one", "one")]);
    fs::remove_file(root.path().join("context.json")).unwrap();
    let error = load_benchmark_manifest(manifest_path).unwrap_err();
    assert!(error.to_string().contains("workspace_context"));
}

#[cfg(unix)]
#[test]
fn manifest_rejects_symlink_escapes_unsafe_links_and_cycles() {
    use std::os::unix::fs::symlink;

    let (root, manifest_path) = benchmark(&[("one", "one")]);
    fs::write(root.path().join("secret"), "outside fixture").unwrap();
    symlink("../../secret", root.path().join("fixture/one/escape")).unwrap();
    let error = load_benchmark_manifest(&manifest_path).unwrap_err();
    assert!(error.to_string().contains("escapes the fixture directory"));

    fs::remove_file(root.path().join("fixture/one/escape")).unwrap();
    symlink("README.md", root.path().join("fixture/one/alias")).unwrap();
    let error = load_benchmark_manifest(&manifest_path).unwrap_err();
    assert!(error.to_string().contains("symlinks are not allowed"));

    fs::remove_file(root.path().join("fixture/one/alias")).unwrap();
    symlink("..", root.path().join("fixture/one/cycle")).unwrap();
    let error = load_benchmark_manifest(manifest_path).unwrap_err();
    assert!(matches!(
        error,
        BenchmarkManifestError::DirectoryCycle { .. }
    ));
}

#[test]
fn checked_in_controlled_profile_resolves_fixture_provider_and_exact_patch() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml");
    let manifest = load_benchmark_manifest(manifest_path).unwrap();

    assert_eq!(manifest.manifest().name, "codebase-memory-routing-repair");
    assert!(
        manifest
            .condition_fixture_provider_path()
            .unwrap()
            .is_file()
    );
    assert!(
        manifest
            .condition_disabled_jig_script_path()
            .unwrap()
            .is_file()
    );
    assert!(
        manifest
            .condition_unavailable_jig_script_path()
            .unwrap()
            .is_file()
    );
    assert!(manifest.expected_patch_path().unwrap().is_file());
    assert_eq!(manifest.manifest().graph_decision_targets.len(), 3);
}

#[test]
fn every_repetition_is_isolated_and_has_reproducible_baselines() {
    let (root, manifest_path) = benchmark(&[("one", "one"), ("two", "two")]);
    let source_before = fs::read(root.path().join("fixture/one/README.md")).unwrap();
    let manifest = load_benchmark_manifest(manifest_path).unwrap();

    let first = prepare_benchmark_workspace(&manifest, 1).unwrap();
    assert_eq!(first.baselines().len(), 2);
    for baseline in first.baselines() {
        assert_eq!(baseline.sha.len(), 40);
        assert!(baseline.sha.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let repository = first.root().join(&baseline.dir);
        assert!(repository.join(".git").is_dir());
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repository)
            .output()
            .unwrap();
        assert!(head.status.success());
        assert_eq!(String::from_utf8(head.stdout).unwrap().trim(), baseline.sha);
    }
    fs::write(first.root().join("one/README.md"), "mutated\n").unwrap();

    let second = prepare_benchmark_workspace(&manifest, 2).unwrap();
    assert_ne!(first.root(), second.root());
    assert_eq!(
        fs::read(second.root().join("one/README.md")).unwrap(),
        source_before
    );
    assert_eq!(first.baselines(), second.baselines());
    assert_eq!(
        fs::read(root.path().join("fixture/one/README.md")).unwrap(),
        source_before
    );
    assert!(!root.path().join("fixture/one/.git").exists());
}

#[cfg(unix)]
#[test]
fn prepared_context_directories_cannot_be_replaced_by_escape_links() {
    use std::os::unix::fs::symlink;

    let (_root, manifest_path) = benchmark(&[("one", "one")]);
    let manifest = load_benchmark_manifest(manifest_path).unwrap();
    let workspace = prepare_benchmark_workspace(&manifest, 1).unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::remove_dir_all(workspace.root().join("one")).unwrap();
    symlink(external.path(), workspace.root().join("one")).unwrap();

    let error = workspace.verify_context_directories().unwrap_err();
    assert!(error.to_string().contains("must remain a real directory"));
}

#[test]
fn artifact_layout_and_snapshots_are_deterministic_and_exclude_secrets() {
    const SENTINEL: &str = "TEMPER_BENCHMARK_SECRET_SENTINEL_4fdd9e";

    let (root, manifest_path) = benchmark(&[("one", "one")]);
    // An undeclared sibling models credentials or other ambient host state.
    fs::write(root.path().join("credentials.env"), SENTINEL).unwrap();
    let manifest = load_benchmark_manifest(manifest_path).unwrap();
    let workspace = prepare_benchmark_workspace(&manifest, 1).unwrap();
    let output = root.path().join("artifacts");
    let layout = BenchmarkArtifactLayout::create(&output, 2).unwrap();
    let paths = layout.snapshot_inputs(1, &manifest, &workspace).unwrap();

    assert_eq!(layout.aggregate_json, output.join("aggregate.json"));
    assert_eq!(layout.aggregate_markdown, output.join("aggregate.md"));
    assert_eq!(paths.root, output.join("repetitions/001"));
    assert_eq!(paths.manifest_snapshot, paths.root.join("manifest.toml"));
    assert_eq!(paths.expected_patch, paths.root.join("expected.patch"));
    assert_eq!(
        paths.workspace_context_snapshot,
        paths.root.join("workspace-context.json")
    );
    assert_eq!(paths.baselines, paths.root.join("baselines.json"));
    assert_eq!(paths.canonical_trace, paths.root.join("trace.export.jsonl"));
    assert_eq!(
        paths.workspace_result,
        paths.root.join("workspace-result.json")
    );
    assert_eq!(paths.run_json, paths.root.join("run.json"));
    assert_eq!(paths.run_markdown, paths.root.join("run.md"));
    assert_eq!(
        paths.validation_evidence,
        paths.root.join("validation.json")
    );
    assert_eq!(paths.diff_statistics, paths.root.join("diff.json"));
    assert_eq!(
        layout.repetition(2).unwrap().root,
        output.join("repetitions/002")
    );

    let baseline: serde_json::Value =
        serde_json::from_slice(&fs::read(&paths.baselines).unwrap()).unwrap();
    assert_eq!(baseline["version"], BASELINE_SNAPSHOT_VERSION);
    assert_eq!(baseline["repetition"], 1);
    assert_eq!(
        baseline["repositories"][0]["sha"],
        workspace.baselines()[0].sha
    );

    for file in files_below(layout.root()) {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL),
            "secret leaked into {}",
            file.display()
        );
    }
}

fn files_below(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_below(&path));
        } else {
            files.push(path);
        }
    }
    files
}
