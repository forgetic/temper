//! Creation proof reusing the unchanged graph-consumption provider transcript.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, StopReason, Turn};
use jig_server::FakeLlm;

use super::{ModelObservations, mapped_graph_consumption_fake};
use crate::live_manifest::patch_creation::{CREATED_FILE, CREATED_SOURCE};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    mapped_graph_consumption_fake::start_with_reply(request_count, observations, reply)
}

fn reply(view: &RequestView) -> Reply {
    if view.prior_tool_results != 8 {
        return mapped_graph_consumption_fake::reply(view);
    }
    Reply {
        turns: vec![Turn::ToolCall {
            id: "patch-read-primary-and-create-regression".into(),
            name: "apply_patch".into(),
            args: serde_json::json!({"patch": patch()}),
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn patch() -> String {
    let primary = "diff --git a/demo/src/lib.rs b/demo/src/lib.rs\n--- a/demo/src/lib.rs\n+++ b/demo/src/lib.rs\n@@ -1,9 +1,5 @@\n pub mod caller;\n \n-pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, attempt: u32) -> &'a str {\n-    if attempt == 0 {\n-        preferred.unwrap_or(value)\n-    } else {\n-        value\n-    }\n+pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, _attempt: u32) -> &'a str {\n+    preferred.unwrap_or(value)\n }\n";
    let lines = CREATED_SOURCE.lines().count();
    let additions = CREATED_SOURCE
        .lines()
        .map(|line| format!("+{line}\n"))
        .collect::<String>();
    format!(
        "{primary}diff --git a/demo/{CREATED_FILE} b/demo/{CREATED_FILE}\nnew file mode 100644\n--- /dev/null\n+++ b/demo/{CREATED_FILE}\n@@ -0,0 +1,{lines} @@\n{additions}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_patch_applies_to_the_inherited_seed_and_creates_exact_regression_bytes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("demo/src/lib.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/mapped-live-graph-consumption/repo/src/lib.rs"
            )),
        )
        .unwrap();
        std::fs::write(root.path().join("patch.diff"), patch()).unwrap();
        for arguments in [
            ["apply", "--check", "patch.diff"].as_slice(),
            ["apply", "patch.diff"].as_slice(),
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(root.path())
                    .args(arguments)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("demo").join(CREATED_FILE)).unwrap(),
            CREATED_SOURCE
        );
        assert!(
            !std::fs::read_to_string(source)
                .unwrap()
                .contains("if attempt")
        );
    }
}
