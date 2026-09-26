//! Companion authority requires its own successful ordinary read.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::{Value, json};
use temper_agent_core::DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE;

use super::mapped_patch_creation_fake::PRIMARY_PATCH;
use super::{ModelObservations, mapped_graph_consumption_fake, messages_contain};
use crate::live_manifest::companion_read::{COMPANION_AFTER, COMPANION_BEFORE};

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    mapped_graph_consumption_fake::start_with_reply(request_count, observations, reply)
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0..=7 => mapped_graph_consumption_fake::reply(view),
        8 => tool(
            "patch-unread-companion-denied",
            "apply_patch",
            json!({"patch": patch()}),
        ),
        9 => {
            assert!(messages_contain(
                view,
                DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE
            ));
            tool(
                "read-existing-companion",
                "read",
                json!({"path":"demo/README.md"}),
            )
        }
        10 => tool(
            "patch-read-primary-and-companion",
            "apply_patch",
            json!({"patch":patch()}),
        ),
        11 => tool(
            "validate-minimal-mapped-repair",
            "bash",
            json!({
                "command":"cd demo && cargo fmt --check && cargo test --quiet", "timeout":60,
            }),
        ),
        12 => tool(
            "submit-mapped-graph-repair",
            "submit_for_pr",
            json!({
                "summary":"Consumed the mapped multi-part current-root lineage before the minimal repair.",
            }),
        ),
        13 => Reply::text(
            r##"{"title":"Keep preferred dispatch and its companion documentation","body":"# Implementation report\nConsumed the mapped multi-part current-root lineage before the minimal repair. `cargo fmt --check` and `cargo test --quiet` pass.","summary":"Read the primary and existing companion before the cohesive repair."}"##,
        ),
        turn => panic!("unexpected mapped companion-read model turn {turn}"),
    }
}

fn tool(id: &str, name: &str, args: Value) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: id.into(),
            name: name.into(),
            args,
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn patch() -> String {
    let removed = COMPANION_BEFORE
        .lines()
        .map(|line| format!("-{line}\n"))
        .collect::<String>();
    let added = COMPANION_AFTER
        .lines()
        .map(|line| format!("+{line}\n"))
        .collect::<String>();
    format!(
        "{PRIMARY_PATCH}diff --git a/demo/README.md b/demo/README.md\n--- a/demo/README.md\n+++ b/demo/README.md\n@@ -1,{} +1,{} @@\n{removed}{added}",
        COMPANION_BEFORE.lines().count(),
        COMPANION_AFTER.lines().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_patch_changes_two_existing_seed_files_without_creation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("demo/src")).unwrap();
        std::fs::write(
            root.join("demo/src/lib.rs"),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/mapped-live-graph-consumption/repo/src/lib.rs"
            )),
        )
        .unwrap();
        std::fs::write(root.join("demo/README.md"), COMPANION_BEFORE).unwrap();
        let patch = patch();
        assert!(!patch.contains("/dev/null"));
        std::fs::write(root.join("patch.diff"), patch).unwrap();
        for args in [
            ["apply", "--check", "patch.diff"].as_slice(),
            ["apply", "patch.diff"].as_slice(),
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert_eq!(
            std::fs::read_to_string(root.join("demo/README.md")).unwrap(),
            COMPANION_AFTER
        );
        assert!(
            !std::fs::read_to_string(root.join("demo/src/lib.rs"))
                .unwrap()
                .contains("if attempt")
        );
    }
}
