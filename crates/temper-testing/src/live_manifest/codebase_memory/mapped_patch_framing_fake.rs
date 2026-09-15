//! Recoverable patch envelopes share the complete historical graph transcript.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView};
use jig_server::FakeLlm;

use super::{ModelObservations, mapped_graph_consumption_fake, mapped_patch_creation_fake};
use crate::live_manifest::patch_creation::{CREATED_FILE, CREATED_SOURCE};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    mapped_graph_consumption_fake::start_with_reply(request_count, observations, reply)
}

fn reply(view: &RequestView) -> Reply {
    mapped_patch_creation_fake::reply_with_patch(
        view,
        "patch-recoverable-framing-and-create-regression",
        patch,
    )
}

fn patch() -> String {
    let creation_header = format!("diff --git a/demo/{CREATED_FILE} b/demo/{CREATED_FILE}");
    let creation_hunk = format!("@@ -0,0 +1,{} @@", CREATED_SOURCE.lines().count());
    mapped_patch_creation_fake::patch()
        .replace(
            "@@ -1,9 +1,5 @@\n pub mod caller;\n",
            "@@ -1 +1 @@\n-pub mod caller;\n+pub mod caller; // Keep caller routing public.\n@@ -2,80 +2,40 @@\n",
        )
        .replace(&creation_header, &format!("  {creation_header}"))
        .replace(&creation_hunk, "@@ -0,0 +1,80 @@")
        .replace("new file mode 100644\n", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn framing_bundle_preserves_creation_proof_and_uses_its_own_mapping() {
        let scenarios = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
        let bundle =
            crate::live_manifest::ScenarioBundle::load(scenarios.join("mapped-live-patch-framing"))
                .unwrap();
        let creation = crate::live_manifest::ScenarioBundle::load(
            scenarios.join("mapped-live-patch-creation"),
        )
        .unwrap();
        assert_eq!(bundle.repo.seed_path, creation.repo.seed_path);
        assert_eq!(
            bundle.resolved_manifest["expect"]["count"],
            creation.resolved_manifest["expect"]["count"]
        );
        assert_eq!(
            bundle.resolved_manifest["validation"]["feature"].as_str(),
            Some("ai/temper#1305")
        );
        assert_eq!(
            creation.resolved_manifest["validation"]["feature"].as_str(),
            Some("ai/temper#1303")
        );
        assert_eq!(
            bundle.jig_script_path().file_name().unwrap(),
            "mapped-live-patch-framing.json"
        );
        assert!(!bundle.repo.seed_path.join(CREATED_FILE).exists());
    }

    #[test]
    fn framing_fixture_requires_recovery_before_git_can_apply_it() {
        let patch = patch();
        assert!(patch.contains("\n  diff --git a/demo/tests/created_dispatch.rs"));
        assert!(patch.contains(
            "@@ -1 +1 @@\n-pub mod caller;\n+pub mod caller; // Keep caller routing public.\n@@"
        ));
        assert!(patch.contains("@@ -2,80 +2,40 @@"));
        assert!(patch.contains("@@ -0,0 +1,80 @@"));
        assert!(!patch.contains("new file mode"));
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("demo/src/lib.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/mapped-live-graph-consumption/repo/src/lib.rs"
            )),
        )
        .unwrap();
        let before = std::fs::read(&source).unwrap();
        std::fs::write(temp.path().join("patch.diff"), patch).unwrap();
        let output = std::process::Command::new("git")
            .current_dir(temp.path())
            .args(["apply", "--check", "patch.diff"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(std::fs::read(source).unwrap(), before);
        assert!(!temp.path().join("demo").join(CREATED_FILE).exists());
    }
}
