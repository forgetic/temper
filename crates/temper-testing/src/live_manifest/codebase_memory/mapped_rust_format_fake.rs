//! Explicit formatting preserves exact-read authority after verified creation.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use jig_core::{Reply, RequestView};
use jig_server::FakeLlm;
use serde_json::json;
use temper_agent_core::DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE;

use super::mapped_graph_consumption_fake::tool_reply;
use super::{
    ModelObservations, mapped_graph_consumption_fake, mapped_patch_creation_fake, messages_contain,
};
use crate::live_manifest::patch_creation::CREATED_FILE;

const UNFORMATTED_PRIMARY: &str = "pub mod caller;\n\npub fn choose_dispatch<'a>(value:&'a str,preferred:Option<&'a str>,_attempt:u32)->&'a str{preferred.unwrap_or(value)}\n";
const UNFORMATTED_CREATED: &str = "use mapped_live_graph_consumption_fixture::choose_dispatch;\n\n#[test]\nfn a_new_regression_keeps_preferred_dispatch_across_retries(){for attempt in [0,1,4]{assert_eq!(choose_dispatch(\"raw\",Some(\"stable\"),attempt),\"stable\");}}\n";

pub(super) fn start(
    request_count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    mapped_graph_consumption_fake::start_with_reply(request_count, observations, reply)
}

fn reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0..=7 => mapped_graph_consumption_fake::reply(view),
        8 => mapped_patch_creation_fake::reply_with_patch(
            view,
            "patch-unformatted-primary-and-create-regression",
            patch,
        ),
        9 => format_reply("format-unread-created-regression-denied"),
        10 => {
            assert!(messages_contain(
                view,
                DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE
            ));
            tool_reply(
                "read-created-regression-before-format",
                "read",
                json!({"path":format!("demo/{CREATED_FILE}")}),
            )
        }
        11 => format_reply("format-read-primary-and-created-regression"),
        12 => tool_reply(
            "validate-minimal-mapped-repair",
            "bash",
            json!({"command":"cd demo && cargo fmt --check && cargo test --quiet", "timeout":60}),
        ),
        13 => tool_reply(
            "submit-mapped-graph-repair",
            "submit_for_pr",
            json!({"summary":"Read every explicit Rust formatting target before the cohesive repair."}),
        ),
        14 => Reply::text(
            r##"{"title":"Keep preferred dispatch with a formatted regression","body":"# Implementation report\nConsumed the mapped graph lineage, read both explicit Rust targets, and formatted the cohesive repair. `cargo fmt --check` and `cargo test --quiet` pass.","summary":"Formatted only the read primary and created regression."}"##,
        ),
        turn => panic!("unexpected mapped Rust-format model turn {turn}"),
    }
}

fn format_reply(id: &str) -> Reply {
    tool_reply(
        id,
        "format_rust",
        json!({"paths":["demo/src/lib.rs", format!("demo/{CREATED_FILE}")], "edition":"2021"}),
    )
}

fn patch() -> String {
    let seed = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenarios/mapped-live-graph-consumption/repo/src/lib.rs"
    ));
    let removed = seed
        .lines()
        .map(|line| format!("-{line}\n"))
        .collect::<String>();
    let primary = UNFORMATTED_PRIMARY
        .lines()
        .map(|line| format!("+{line}\n"))
        .collect::<String>();
    let created = UNFORMATTED_CREATED
        .lines()
        .map(|line| format!("+{line}\n"))
        .collect::<String>();
    format!(
        "diff --git a/demo/src/lib.rs b/demo/src/lib.rs\n--- a/demo/src/lib.rs\n+++ b/demo/src/lib.rs\n@@ -1,{} +1,{} @@\n{removed}{primary}diff --git a/demo/{CREATED_FILE} b/demo/{CREATED_FILE}\nnew file mode 100644\n--- /dev/null\n+++ b/demo/{CREATED_FILE}\n@@ -0,0 +1,{} @@\n{created}",
        seed.lines().count(),
        UNFORMATTED_PRIMARY.lines().count(),
        UNFORMATTED_CREATED.lines().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_manifest::patch_creation::{CREATED_SOURCE, FORMATTED_PRIMARY_SOURCE};
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn formatting_bundle_preserves_graph_counts_and_maps_the_explicit_tool() {
        let scenarios = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
        let load = |name| crate::live_manifest::ScenarioBundle::load(scenarios.join(name)).unwrap();
        let bundle = load("mapped-live-rust-format");
        let creation = load("mapped-live-patch-creation");
        assert_eq!(bundle.repo.seed_path, creation.repo.seed_path);
        assert_eq!(
            bundle.resolved_manifest["expect"]["count"],
            creation.resolved_manifest["expect"]["count"]
        );
        assert_eq!(
            bundle.resolved_manifest["validation"]["feature"].as_str(),
            Some("ai/temper#1314")
        );
        assert_eq!(
            bundle.jig_script_path().file_name().unwrap(),
            "mapped-live-rust-format.json"
        );
        assert!(!bundle.repo.seed_path.join(CREATED_FILE).exists());
    }

    #[test]
    fn formatting_fixture_requires_both_explicit_files_to_reach_the_expected_bytes() {
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
        std::fs::write(temp.path().join("patch.diff"), patch()).unwrap();
        assert!(
            Command::new("git")
                .current_dir(temp.path())
                .args(["apply", "patch.diff"])
                .status()
                .unwrap()
                .success()
        );
        for (path, unformatted, formatted) in [
            ("src/lib.rs", UNFORMATTED_PRIMARY, FORMATTED_PRIMARY_SOURCE),
            (CREATED_FILE, UNFORMATTED_CREATED, CREATED_SOURCE),
        ] {
            let actual = std::fs::read_to_string(temp.path().join("demo").join(path)).unwrap();
            assert_eq!(actual, unformatted);
            assert_ne!(actual, formatted);
            let mut child = Command::new("rustfmt")
                .args([
                    "--edition",
                    "2021",
                    "--emit",
                    "stdout",
                    "--config",
                    "skip_children=true",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(actual.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, formatted.as_bytes());
        }
    }
}
