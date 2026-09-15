//! Read back the created regression from the actual merged scenario commit.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use super::process::run_git_with_token;
use super::{FinalStateEvidence, ScenarioBundle};

pub(super) const FORMATTED_PRIMARY_SOURCE: &str = "pub mod caller;\n\npub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n";
pub(super) const FRAMED_PRIMARY_SOURCE: &str = "pub mod caller; // Keep caller routing public.\n\npub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n";
pub(super) const CREATED_FILE: &str = "tests/created_dispatch.rs";
pub(super) const CREATED_SOURCE: &str = "use mapped_live_graph_consumption_fixture::choose_dispatch;\n\n#[test]\nfn a_new_regression_keeps_preferred_dispatch_across_retries() {\n    for attempt in [0, 1, 4] {\n        assert_eq!(choose_dispatch(\"raw\", Some(\"stable\"), attempt), \"stable\");\n    }\n}\n";

pub(super) fn verify_merged(
    scenario: &ScenarioBundle,
    workspace: &Path,
    token: &str,
    final_state: &FinalStateEvidence,
    log: &Path,
) -> Result<(), String> {
    if !matches!(
        scenario.scenario_path.file_name().and_then(|s| s.to_str()),
        Some(
            "mapped-live-patch-creation" | "mapped-live-patch-framing" | "mapped-live-rust-format"
        )
    ) {
        return Ok(());
    }
    if scenario.repo.seed_path.join(CREATED_FILE).exists() {
        return Err("creation scenario destination must be absent from its seed".into());
    }
    let merged = final_state
        .pull_request
        .merged_sha
        .as_deref()
        .ok_or("creation scenario requires an actual merge SHA")?;
    let checkout = workspace.join("repo-seed").join(&scenario.repo.name);
    run_git_with_token(
        &checkout,
        token,
        &["fetch", "--quiet", "origin", &scenario.repo.default_branch],
        log,
        "fetch merged creation scenario default branch",
    )?;
    if git_output(&checkout, &["rev-parse", "FETCH_HEAD"])? != format!("{merged}\n").as_bytes() {
        return Err("creation scenario default branch is not the recorded merged commit".into());
    }
    verify_created_blob(&checkout, merged)?;
    if scenario.scenario_path.file_name().and_then(|s| s.to_str())
        == Some("mapped-live-patch-framing")
    {
        verify_blob(&checkout, merged, "src/lib.rs", FRAMED_PRIMARY_SOURCE)?;
    }
    if scenario.scenario_path.file_name().and_then(|s| s.to_str())
        == Some("mapped-live-rust-format")
    {
        verify_blob(&checkout, merged, "src/lib.rs", FORMATTED_PRIMARY_SOURCE)?;
    }
    verify_changed_paths(
        &checkout,
        merged,
        &["Cargo.lock", "src/lib.rs", CREATED_FILE],
    )?;
    writeln!(OpenOptions::new().append(true).open(log).map_err(|e| e.to_string())?,
        "patch-creation-fact {{\"checkpoint\":\"absent-seed-file-matches-merged-bytes\",\"passed\":true}}")
        .map_err(|e| e.to_string())
}

fn verify_created_blob(checkout: &Path, merged: &str) -> Result<(), String> {
    verify_blob(checkout, merged, CREATED_FILE, CREATED_SOURCE)
}

fn verify_blob(checkout: &Path, merged: &str, path: &str, expected: &str) -> Result<(), String> {
    let blob = git_output(checkout, &["show", &format!("{merged}:{path}")])?;
    if blob != expected.as_bytes() {
        return Err("mapped patch file does not match expected merged bytes".into());
    }
    Ok(())
}

pub(super) fn verify_changed_paths(
    checkout: &Path,
    merged: &str,
    expected: &[&str],
) -> Result<(), String> {
    let changed = git_output(
        checkout,
        &["diff", "--name-only", &format!("{merged}^"), merged],
    )?;
    if changed != format!("{}\n", expected.join("\n")).as_bytes() {
        return Err("mapped patch delivery contains unexpected changed paths".into());
    }
    Ok(())
}

fn git_output(checkout: &Path, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .current_dir(checkout)
        .args(arguments)
        .output()
        .map_err(|_| "cannot read creation scenario commit")?;
    if !output.status.success() {
        return Err("cannot read creation scenario commit".into());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_creation_bundle_keeps_historical_graph_counts_and_missing_destination() {
        let scenarios = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
        let bundle = ScenarioBundle::load(scenarios.join("mapped-live-patch-creation")).unwrap();
        let historical =
            ScenarioBundle::load(scenarios.join("mapped-live-graph-consumption")).unwrap();
        assert_eq!(
            bundle.jig_script_path().file_name().unwrap(),
            "mapped-live-patch-creation.json"
        );
        assert_eq!(bundle.repo.seed_path, historical.repo.seed_path);
        assert!(!bundle.repo.seed_path.join(CREATED_FILE).exists());
        assert_eq!(
            bundle.resolved_manifest["expect"]["count"],
            historical.resolved_manifest["expect"]["count"]
        );
        assert_eq!(
            bundle.resolved_manifest["validation"]["feature"].as_str(),
            Some("ai/temper#1303")
        );
        assert_eq!(
            historical.resolved_manifest["validation"]["feature"].as_str(),
            Some("ai/temper#1009")
        );
        assert_eq!(
            bundle.resolved_manifest["assertions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(bundle.repo.ci_source.contains("cargo test --quiet"));
    }

    #[test]
    fn merged_path_check_requires_the_exact_patch_and_generated_lockfile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git_output(root, &["init", "--quiet"]).unwrap();
        let commit = || {
            git_output(root, &["add", "."]).unwrap();
            git_output(
                root,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "fixture",
                ],
            )
            .unwrap();
        };
        commit();
        for path in ["Cargo.lock", "src/lib.rs", CREATED_FILE] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), "fixture\n").unwrap();
        }
        commit();
        let expected = ["Cargo.lock", "src/lib.rs", CREATED_FILE];
        verify_changed_paths(root, "HEAD", &expected).unwrap();
        assert!(verify_changed_paths(root, "HEAD", &["src/lib.rs", CREATED_FILE]).is_err());
        assert!(
            verify_changed_paths(root, "HEAD", &["Cargo.lock", "README.md", "src/lib.rs"]).is_err()
        );
        std::fs::write(root.join("unexpected.txt"), "extra\n").unwrap();
        commit();
        assert!(verify_changed_paths(root, "HEAD", &expected).is_err());
        verify_changed_paths(root, "HEAD^", &expected).unwrap();
    }

    #[test]
    fn merged_creation_check_reads_commit_bytes_not_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git_output(root, &["init", "--quiet"]).unwrap();
        std::fs::create_dir(root.join("tests")).unwrap();
        std::fs::write(root.join(CREATED_FILE), CREATED_SOURCE).unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), FRAMED_PRIMARY_SOURCE).unwrap();
        git_output(root, &["add", CREATED_FILE, "src/lib.rs"]).unwrap();
        git_output(
            root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "created regression",
            ],
        )
        .unwrap();
        std::fs::write(root.join(CREATED_FILE), "wrong working tree bytes\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "wrong primary bytes\n").unwrap();
        verify_created_blob(root, "HEAD").unwrap();
        verify_blob(root, "HEAD", "src/lib.rs", FRAMED_PRIMARY_SOURCE).unwrap();
        git_output(root, &["add", CREATED_FILE, "src/lib.rs"]).unwrap();
        git_output(
            root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "wrong committed bytes",
            ],
        )
        .unwrap();
        assert!(verify_created_blob(root, "HEAD").is_err());
        assert!(verify_blob(root, "HEAD", "src/lib.rs", FRAMED_PRIMARY_SOURCE).is_err());
        verify_created_blob(root, "HEAD^").unwrap();
        verify_blob(root, "HEAD^", "src/lib.rs", FRAMED_PRIMARY_SOURCE).unwrap();
        assert!(verify_created_blob(root, "missing-ref").is_err());
    }
}
