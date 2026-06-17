// SPDX-License-Identifier: MPL-2.0

use crate::{WorkspaceManifest, WorkspaceRepo};

use super::sample_manifest;

#[test]
fn single_repo_manifest_is_one_writable_primary() {
    let manifest = WorkspaceManifest::single(
        "ai/temper",
        "temper",
        "main",
        "main",
        "agent/pr-for-code-42",
        "pr-for-code-42",
    );
    assert_eq!(manifest.repos.len(), 1);
    let primary = manifest.primary().expect("primary present");
    assert_eq!(primary.repo, "ai/temper");
    assert!(primary.is_writable());
    assert_eq!(manifest.writable().count(), 1);
}

#[test]
fn manifest_round_trips_and_reports_writable_repos() {
    let manifest = sample_manifest();
    assert_eq!(
        manifest
            .writable()
            .map(|repo| repo.repo.as_str())
            .collect::<Vec<_>>(),
        vec!["ai/temper", "ai/smith"]
    );
    assert_eq!(manifest.primary().unwrap().repo, "ai/temper");

    let value = serde_json::to_value(&manifest).expect("manifest serializes");
    assert_eq!(value["coordination_key"], "coord-for-code-42");
    assert_eq!(value["repos"][2]["access"], "read_only");
    assert_eq!(value["repos"][2].get("branch_hint"), None);
    // Landing order: smith lands after temper; independent repos omit it.
    assert_eq!(value["repos"][0].get("depends_on"), None);
    assert_eq!(
        value["repos"][1]["depends_on"],
        serde_json::json!(["ai/temper"])
    );
    let decoded: WorkspaceManifest = serde_json::from_value(value).expect("manifest parses");
    assert_eq!(decoded, manifest);
}

#[test]
fn workspace_repo_owner_name_splits_or_rejects() {
    let writable = &sample_manifest().repos[0];
    assert_eq!(writable.owner_name(), Some(("ai", "temper")));
    let malformed = WorkspaceRepo {
        repo: "ai/temper/extra".to_string(),
        ..writable.clone()
    };
    assert_eq!(malformed.owner_name(), None);
}
