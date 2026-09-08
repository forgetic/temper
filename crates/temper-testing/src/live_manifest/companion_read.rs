//! Read back the edited existing companion from the actual merged commit.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use super::process::run_git_with_token;
use super::{FinalStateEvidence, ScenarioBundle};

pub(super) const COMPANION_FILE: &str = "README.md";
pub(super) const COMPANION_BEFORE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/mapped-live-graph-consumption/repo/README.md"
));
pub(super) const COMPANION_AFTER: &str = "# Mapped live graph-consumption fixture\n\nPreferred dispatch remains stable on the initial attempt and every retry.\nThe live provider's typed current-root evidence precedes the implementation\nread, companion documentation read, and cohesive repair.\n";

pub(super) fn verify_merged(
    scenario: &ScenarioBundle,
    workspace: &Path,
    token: &str,
    final_state: &FinalStateEvidence,
    log: &Path,
) -> Result<(), String> {
    if scenario.scenario_path.file_name().and_then(|s| s.to_str())
        != Some("mapped-live-companion-read")
    {
        return Ok(());
    }
    if std::fs::read_to_string(scenario.repo.seed_path.join(COMPANION_FILE))
        .map_err(|_| "companion scenario requires its existing seed file")?
        != COMPANION_BEFORE
        || COMPANION_BEFORE == COMPANION_AFTER
    {
        return Err("companion scenario must change its expected existing seed file".into());
    }
    let merged = final_state
        .pull_request
        .merged_sha
        .as_deref()
        .ok_or("companion scenario requires an actual merge SHA")?;
    let checkout = workspace.join("repo-seed").join(&scenario.repo.name);
    run_git_with_token(
        &checkout,
        token,
        &["fetch", "--quiet", "origin", &scenario.repo.default_branch],
        log,
        "fetch merged companion scenario default branch",
    )?;
    if git_output(&checkout, &["rev-parse", "FETCH_HEAD"])? != format!("{merged}\n").as_bytes() {
        return Err("companion scenario default branch is not the recorded merged commit".into());
    }
    verify_companion_blob(&checkout, merged)?;
    writeln!(OpenOptions::new().append(true).open(log).map_err(|e| e.to_string())?,
        "companion-read-fact {{\"checkpoint\":\"existing-seed-file-matches-merged-change\",\"passed\":true}}")
        .map_err(|e| e.to_string())
}

fn verify_companion_blob(checkout: &Path, merged: &str) -> Result<(), String> {
    let blob = git_output(checkout, &["show", &format!("{merged}:{COMPANION_FILE}")])?;
    if blob != COMPANION_AFTER.as_bytes() {
        return Err("companion documentation does not match expected merged bytes".into());
    }
    Ok(())
}

fn git_output(checkout: &Path, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .current_dir(checkout)
        .args(arguments)
        .output()
        .map_err(|_| "cannot read companion scenario commit")?;
    if !output.status.success() {
        return Err("cannot read companion scenario commit".into());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_companion_bundle_keeps_graph_counts_and_existing_seed() {
        let scenarios = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
        let bundle = ScenarioBundle::load(scenarios.join("mapped-live-companion-read")).unwrap();
        let historical =
            ScenarioBundle::load(scenarios.join("mapped-live-graph-consumption")).unwrap();
        assert_eq!(
            bundle.jig_script_path().file_name().unwrap(),
            "mapped-live-companion-read.json"
        );
        assert_eq!(bundle.repo.seed_path, historical.repo.seed_path);
        assert_eq!(
            std::fs::read_to_string(bundle.repo.seed_path.join(COMPANION_FILE)).unwrap(),
            COMPANION_BEFORE
        );
        assert_eq!(
            bundle.resolved_manifest["expect"]["count"],
            historical.resolved_manifest["expect"]["count"]
        );
        assert_eq!(
            bundle.resolved_manifest["validation"]["feature"].as_str(),
            Some("ai/temper#1304")
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
    fn merged_companion_check_reads_commit_bytes_not_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git_output(root, &["init", "--quiet"]).unwrap();
        std::fs::write(root.join(COMPANION_FILE), COMPANION_AFTER).unwrap();
        git_output(root, &["add", COMPANION_FILE]).unwrap();
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
                "companion documentation",
            ],
        )
        .unwrap();
        std::fs::write(root.join(COMPANION_FILE), "wrong working tree bytes\n").unwrap();
        verify_companion_blob(root, "HEAD").unwrap();
        git_output(root, &["add", COMPANION_FILE]).unwrap();
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
        assert!(verify_companion_blob(root, "HEAD").is_err());
        verify_companion_blob(root, "HEAD^").unwrap();
        assert!(verify_companion_blob(root, "missing-ref").is_err());
    }
}
