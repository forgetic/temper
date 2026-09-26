use super::*;
use crate::live_manifest::codebase_memory::{stable_lifecycle_fake, write_fake_mcp};
use jig_core::{Dialect, RequestView, ScriptFile, Turn, ViewMessage};
use serde_json::{Value, json};

fn fixture_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/codebase-memory-agent")
}

fn script() -> jig_core::Script {
    ScriptFile::load(fixture_root().join("jig/codebase-memory-agent.json"))
        .expect("basic Jig script")
        .into_script()
}

fn results() -> Vec<Value> {
    let source = |symbol: &str, path: &str| {
        let actual = std::fs::read_to_string(fixture_root().join("repo").join(path)).unwrap();
        let legacy = std::fs::read_to_string(
            fixture_root()
                .join("../codebase-memory-graph-consumption/repo")
                .join(path),
        )
        .unwrap();
        assert_eq!(
            actual, legacy,
            "shared observer requires the actual seed bytes"
        );
        json!({"qualified_name":symbol,"file_path":path,"source":actual})
    };
    vec![
        json!({"results":[{"results":[
            {"qualifiedName":"retry_worker_topic","file_path":"src/lib.rs"},
            {"qualifiedName":"alias_retries_keep_the_original_ordered_worker","file_path":"tests/retry_affinity.rs"}
        ]}]}),
        json!({"results":[{"qualified_name":"retry_worker_topic","file_path":"src/lib.rs",
            "related_source_references":[{"qualifiedName":"dispatch"}]}]}),
        source("retry_worker_topic", "src/lib.rs"),
        json!({"function":{"qualified_name":"retry_worker_topic"},"direction":"inbound","complete":true,
            "callers":[{"qualified_name":"dispatch","file_path":"src/caller.rs"}],
            "related_sources":[{"qualified_name":"dispatch","file_path":"src/caller.rs"}]}),
        source("dispatch", "src/caller.rs"),
        source(
            "alias_retries_keep_the_original_ordered_worker",
            "tests/retry_affinity.rs",
        ),
    ]
}

fn result_view(results: &[Value]) -> RequestView {
    RequestView::new(
        Dialect::OpenAi,
        None,
        results
            .iter()
            .map(|result| ViewMessage {
                role: "tool".into(),
                content: format!("{result}\n\n[Decision anchor: current-root source]"),
            })
            .collect(),
        results.len(),
    )
}

#[test]
fn checked_in_script_uses_shared_schema_and_writes_the_verified_match() {
    let script = script();
    let results = results();
    let expected = [
        "codebase_memory_search_graph",
        "codebase_memory_search_code",
        "codebase_memory_get_code_snippet",
        "codebase_memory_trace_path",
        "codebase_memory_get_code_snippet",
        "codebase_memory_get_code_snippet",
        "apply_patch",
        "submit_for_pr",
    ];
    for (index, expected) in expected.iter().enumerate() {
        let reply = stable_lifecycle_fake::reply(&result_view(&results[..index.min(6)]), &script);
        let [Turn::ToolCall { name, args, .. }] = reply.turns.as_slice() else {
            panic!("tool turn")
        };
        assert_eq!(name, expected);
        assert!(
            args.get("project").is_none(),
            "workspace defaults every graph call"
        );
        if index == 1 {
            assert_eq!(args, &json!({"pattern":"retry_worker_topic"}));
        }
        if index == 6 {
            let patch = args["patch"].as_str().unwrap();
            assert!(patch.contains("--- /dev/null\n+++ b/demo/MEMORY_NOTES.md\n"));
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("notes.patch"), patch).unwrap();
            assert!(
                std::process::Command::new("git")
                    .args(["apply", "notes.patch"])
                    .current_dir(root.path())
                    .status()
                    .unwrap()
                    .success()
            );
            let created =
                std::fs::read_to_string(root.path().join("demo/MEMORY_NOTES.md")).unwrap();
            assert!(created.starts_with("memory-guided notes\n"));
            assert_eq!(created.lines().count(), 4);
            for symbol in [
                "retry_worker_topic",
                "dispatch",
                "alias_retries_keep_the_original_ordered_worker",
            ] {
                assert!(patch.contains(symbol));
            }
        }
    }
}

#[test]
fn only_exact_tool_role_source_matches_establish_consumption() {
    let valid = results();
    assert!(stable_lifecycle_fake::verified_result(&result_view(&valid)));
    for index in 0..valid.len() {
        let mut missing = valid.clone();
        missing.remove(index);
        assert!(!stable_lifecycle_fake::verified_result(&result_view(
            &missing
        )));
    }
    for index in [2, 4, 5] {
        for field in ["qualified_name", "file_path", "source"] {
            for wrong in [Value::Null, json!("foreign"), json!("")] {
                let mut foreign = valid.clone();
                foreign[index][field] = wrong;
                foreign[index]["binding"] = json!("current_prepared_checkout");
                assert!(!stable_lifecycle_fake::verified_result(&result_view(
                    &foreign
                )));
            }
        }
    }
    for role in ["assistant", "user", "system"] {
        let mut request = result_view(&valid);
        for message in &mut request.messages {
            message.role = role.into();
        }
        assert!(!stable_lifecycle_fake::verified_result(&request));
    }
    for content in [
        "{malformed",
        "FAKE_MCP_SEARCH_RESULT",
        "{\"binding\":\"current_prepared_checkout\"}",
    ] {
        let mut request = result_view(&valid);
        request.messages[2].content = content.into();
        assert!(!stable_lifecycle_fake::verified_result(&request));
    }
}

#[test]
#[should_panic(expected = "must receive complete graph source before creating notes")]
fn failed_search_cannot_advance_the_script_to_mutation() {
    let script = script();
    let empty = result_view(&[]);
    for _ in 0..7 {
        stable_lifecycle_fake::reply(&empty, &script);
    }
}

fn fixture() -> (tempfile::TempDir, FakeMcpServer, Vec<McpToolCallEvidence>) {
    let root = tempfile::tempdir().unwrap();
    let project = "temper-v1-basic";
    let mcp = write_fake_mcp(
        root.path(),
        project,
        Some("stable-lifecycle"),
        &[],
        &[],
        0,
        None,
    )
    .unwrap();
    let path = root.path().to_str().unwrap();
    std::fs::write(&mcp.state_path, json!({"projects":{project:{
        "repo_path":path,"binding":"current_prepared_checkout","requested_stable_project":project
    }},"counters":{"project_creations":1,"rebinds":1},"historical_tokens":{
        "implementation":"retry_worker_topic","caller":"dispatch","focused_test":"alias_retries_keep_the_original_ordered_worker"
    }}).to_string()).unwrap();
    let mut calls = vec![
        ("index_status", json!({"project":project}), true, None, None),
        (
            "index_repository",
            json!({"name":project,"repo_path":path}),
            false,
            Some(0),
            Some("normalized_current_root_upsert"),
        ),
        (
            "index_status",
            json!({"project":project}),
            false,
            None,
            Some("current_root_confirmed"),
        ),
    ]
    .into_iter()
    .map(
        |(name, arguments, is_error, delay_ms, fixture_event)| McpToolCallEvidence {
            name: name.into(),
            arguments,
            is_error,
            delay_ms,
            fixture_event: fixture_event.map(str::to_string),
        },
    )
    .collect::<Vec<_>>();
    let script = script();
    for event in [
        "served_current_root_graph",
        "served_current_root_code_refinement",
        "served_current_root_source",
        "served_current_root_graph_trace",
        "served_current_root_source",
        "served_current_root_source",
    ] {
        let reply = stable_lifecycle_fake::reply(&result_view(&[]), &script);
        let [Turn::ToolCall { name, args, .. }] = reply.turns.as_slice() else {
            panic!("graph turn")
        };
        let mut arguments = args.clone();
        arguments["project"] = json!(project);
        arguments
            .as_object_mut()
            .unwrap()
            .remove("decision_evidence_kind");
        calls.push(McpToolCallEvidence {
            name: name.strip_prefix("codebase_memory_").unwrap().into(),
            arguments,
            is_error: false,
            delay_ms: None,
            fixture_event: Some(event.into()),
        });
    }
    (root, mcp, calls)
}

#[test]
fn basic_inventory_requires_ready_confirmation_and_current_root_isolation() {
    let (_root, mcp, calls) = fixture();
    validate(&mcp, &calls).unwrap();
    for index in 0..calls.len() {
        let mut incomplete = calls.clone();
        incomplete.remove(index);
        assert!(validate(&mcp, &incomplete).is_err());
        let mut foreign = calls.clone();
        foreign[index].arguments["project"] = json!("foreign");
        if index != 1 {
            assert!(validate(&mcp, &foreign).is_err());
        }
        if index > 2 {
            let mut failed = calls.clone();
            failed[index].is_error = true;
            assert!(validate(&mcp, &failed).is_err());
        }
    }
    let mut wrong_root: Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp.state_path).unwrap()).unwrap();
    wrong_root["projects"][&mcp.project]["repo_path"] = json!("foreign-root");
    std::fs::write(&mcp.state_path, wrong_root.to_string()).unwrap();
    assert!(validate(&mcp, &calls).is_err());
}

#[test]
fn basic_observations_require_the_complete_short_delivery() {
    let mut observations = ModelObservations {
        prompt_guidance_seen: true,
        memory_result_seen: true,
        code_refinement_seen: true,
        graph_trace_seen: true,
        current_root_source_seen: true,
        current_root_source_results: 3,
        ..Default::default()
    };
    validate_observations(&observations, 9).unwrap();
    assert!(validate_observations(&observations, 8).is_err());
    observations.current_root_source_results = 2;
    assert!(validate_observations(&observations, 9).is_err());
    observations.current_root_source_results = 3;
    observations.raw_provider_text_seen = true;
    assert!(validate_observations(&observations, 9).is_err());
}
