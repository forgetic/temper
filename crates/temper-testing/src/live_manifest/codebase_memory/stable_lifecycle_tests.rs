use super::*;
use crate::live_manifest::codebase_memory::{stable_lifecycle_fake, write_fake_mcp};
use jig_core::{Dialect, RequestView, ScriptFile, Turn, ViewMessage};
use serde_json::{Value, json};

fn result_view(result: Value) -> RequestView {
    RequestView::new(
        Dialect::OpenAi,
        None,
        vec![
            ViewMessage {
                role: "user".into(),
                content: "ROLE: engineer\nCODEBASE MEMORY".into(),
            },
            ViewMessage {
                role: "tool".into(),
                content: format!("{result}\n\n[Decision anchor: current-root search]"),
            },
        ],
        1,
    )
}

fn valid_result() -> Value {
    json!({"matches":[stable_lifecycle_fake::expected_match()],"total":1,"has_more":false})
}

fn script() -> jig_core::Script {
    ScriptFile::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scenarios/codebase-memory-agent/jig/codebase-memory-agent.json"),
    )
    .expect("basic Jig script")
    .into_script()
}

#[test]
fn checked_in_script_uses_shared_schema_and_writes_the_verified_match() {
    let mut request = result_view(valid_result());
    request.prior_tool_results = 0;
    let first = stable_lifecycle_fake::reply(&request, &script());
    let [Turn::ToolCall { name, args, .. }] = first.turns.as_slice() else {
        panic!("search turn")
    };
    assert_eq!(name, "codebase_memory_search_code");
    assert_eq!(args, &json!({"pattern":PATTERN}));
    request.prior_tool_results = 1;
    let next = stable_lifecycle_fake::reply(&request, &script());
    let [Turn::ToolCall { name, args, .. }] = next.turns.as_slice() else {
        panic!("write turn")
    };
    assert_eq!(name, "write");
    assert_eq!(args["path"], format!("demo/{OUTPUT_FILE}"));
    let expected = stable_lifecycle_fake::expected_match();
    assert_eq!(
        args["content"],
        format!(
            "memory-guided notes\nREADME.md:{}: {}\n",
            expected["line"],
            expected["content"].as_str().unwrap()
        )
    );
}

#[test]
fn only_exact_tool_role_source_matches_establish_consumption() {
    assert!(stable_lifecycle_fake::verified_result(&result_view(
        valid_result()
    )));
    for pointer in [
        "/matches/0/file_path",
        "/matches/0/line",
        "/matches/0/content",
        "/total",
        "/has_more",
    ] {
        for wrong in [Value::Null, json!("foreign"), json!("")] {
            let mut result = valid_result();
            *result.pointer_mut(pointer).unwrap() = wrong;
            result["binding"] = json!("current_prepared_checkout");
            assert!(
                !stable_lifecycle_fake::verified_result(&result_view(result)),
                "{pointer}"
            );
        }
    }
    for role in ["assistant", "user", "system"] {
        let mut request = result_view(valid_result());
        request.messages[1].role = role.into();
        assert!(!stable_lifecycle_fake::verified_result(&request));
    }
    for content in [
        "{malformed",
        "FAKE_MCP_SEARCH_RESULT",
        "{\"binding\":\"current_prepared_checkout\"}",
    ] {
        let mut request = result_view(valid_result());
        request.messages[1].content = content.into();
        assert!(!stable_lifecycle_fake::verified_result(&request));
    }
    let mut duplicate = valid_result();
    duplicate["matches"]
        .as_array_mut()
        .unwrap()
        .push(stable_lifecycle_fake::expected_match());
    assert!(!stable_lifecycle_fake::verified_result(&result_view(
        duplicate
    )));
}

#[test]
#[should_panic(expected = "must receive its verified search source before writing")]
fn failed_search_cannot_advance_the_script_to_mutation() {
    stable_lifecycle_fake::reply(&result_view(json!({"error":"missing source"})), &script());
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
    std::fs::write(
        &mcp.state_path,
        json!({"projects":{project:{
        "repo_path":path,"binding":"current_prepared_checkout","requested_stable_project":project
    }},"counters":{"project_creations":1,"rebinds":1}})
        .to_string(),
    )
    .unwrap();
    let calls = [
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
        (
            "search_code",
            json!({"project":project,"pattern":PATTERN}),
            false,
            None,
            Some("served_stable_current_root_search"),
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
    .collect();
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
    }
    for index in [0, 2, 3] {
        let mut foreign = calls.clone();
        foreign[index].arguments["project"] = json!("foreign");
        assert!(validate(&mcp, &foreign).is_err());
    }
    let mut failed = calls.clone();
    failed[3].is_error = true;
    assert!(validate(&mcp, &failed).is_err());
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
        current_root_source_seen: true,
        current_root_source_results: 1,
        ..Default::default()
    };
    validate_observations(&observations, 4).unwrap();
    assert!(validate_observations(&observations, 3).is_err());
    observations.current_root_source_seen = false;
    assert!(validate_observations(&observations, 4).is_err());
    observations.current_root_source_seen = true;
    observations.raw_provider_text_seen = true;
    assert!(validate_observations(&observations, 4).is_err());
}
