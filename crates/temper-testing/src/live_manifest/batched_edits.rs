//! Exact merged-source evidence for a batch of existing-file edits.

use std::path::Path;

use super::companion_read::git_output;

pub(super) const PRIMARY_BEFORE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/mapped-live-graph-consumption/repo/src/lib.rs"
));
pub(super) const PRIMARY_AFTER: &str = "pub mod caller;\n\npub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n    preferred.unwrap_or(value)\n}\n";

pub(super) fn verify_primary_blob(checkout: &Path, merged: &str) -> Result<(), String> {
    let blob = git_output(checkout, &["show", &format!("{merged}:src/lib.rs")])?;
    if blob != PRIMARY_AFTER.as_bytes() {
        return Err("batched edit primary does not match expected merged bytes".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_manifest::ScenarioBundle;

    #[test]
    fn batched_edit_bundle_preserves_graph_counts_and_existing_sources() {
        let scenarios = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
        let bundle = ScenarioBundle::load(scenarios.join("mapped-live-batched-edits")).unwrap();
        let inherited =
            ScenarioBundle::load(scenarios.join("mapped-live-graph-consumption")).unwrap();
        assert_eq!(bundle.repo.seed_path, inherited.repo.seed_path);
        assert_eq!(
            bundle.resolved_manifest["expect"]["count"],
            inherited.resolved_manifest["expect"]["count"]
        );
        assert_eq!(
            std::fs::read_to_string(bundle.repo.seed_path.join("src/lib.rs")).unwrap(),
            PRIMARY_BEFORE
        );
        assert_ne!(PRIMARY_BEFORE, PRIMARY_AFTER);
        let sequence = bundle.resolved_manifest["expect"]["sequence"]
            .as_array()
            .unwrap();
        let batch = sequence
            .iter()
            .find(|entry| {
                entry["id"].as_str() == Some("typed-source-and-companion-read-before-batched-edits")
            })
            .unwrap();
        let calls = batch["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["fields"]["tool"].as_str() == Some("edit_files"))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["event"].as_str(), Some("tool.error"));
        assert_eq!(calls[1]["event"].as_str(), Some("tool.end"));
    }

    #[test]
    fn batched_edit_primary_check_uses_merged_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("src")).unwrap();
        git_output(root, &["init", "--quiet"]).unwrap();
        std::fs::write(root.join("src/lib.rs"), PRIMARY_AFTER).unwrap();
        commit(root);
        std::fs::write(root.join("src/lib.rs"), PRIMARY_BEFORE).unwrap();
        verify_primary_blob(root, "HEAD").unwrap();
        commit(root);
        assert!(verify_primary_blob(root, "HEAD").is_err());
        verify_primary_blob(root, "HEAD^").unwrap();
        assert!(verify_primary_blob(root, "missing-ref").is_err());
    }

    fn commit(root: &Path) {
        git_output(root, &["add", "src/lib.rs"]).unwrap();
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
                "batch primary",
            ],
        )
        .unwrap();
    }
}
