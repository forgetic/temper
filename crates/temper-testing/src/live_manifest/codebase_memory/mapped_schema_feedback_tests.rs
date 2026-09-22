use super::*;
use jig_core::{Dialect, ViewMessage};
use std::path::Path;

fn request(content: &str, arguments: &str, count: usize) -> RecordedRequest {
    RecordedRequest {
        path: "/chat/completions".into(),
        method: "POST".into(),
        body: json!({"messages":[
            {"role":"assistant","tool_calls":[{"function":{"name":"edit_files","arguments":arguments}}]},
            {"role":"tool","content":content},
        ]})
        .to_string()
        .into_bytes(),
        view: Some(RequestView::new(
            Dialect::OpenAi,
            None,
            vec![ViewMessage {
                role: "tool".into(),
                content: content.into(),
            }],
            count,
        )),
    }
}

#[test]
fn schema_feedback_wire_check_rejects_hidden_arguments_and_missing_hint() {
    validate_requests(&[request(SAFE_FEEDBACK, "{}", 9)]).unwrap();
    assert!(validate_requests(&[request("generic error", "{}", 9)]).is_err());
    assert!(validate_requests(&[request(SAFE_FEEDBACK, "{}", 10)]).is_err());
    for sentinel in [UNKNOWN_KEY, REPLACEMENT_VALUE] {
        // The projected messages are safe; only the raw assistant arguments leak.
        assert!(validate_requests(&[request(SAFE_FEEDBACK, sentinel, 9)]).is_err());
        assert!(
            validate_requests(&[
                request(SAFE_FEEDBACK, "{}", 9),
                request(SAFE_FEEDBACK, sentinel, 10),
            ])
            .is_err()
        );
    }
}

#[test]
fn schema_feedback_fixture_retries_the_existing_batch_after_the_hint() {
    let malformed = request("", "{}", 8);
    let reply = super::reply(malformed.view.as_ref().unwrap());
    let Turn::ToolCall { name, args, .. } = &reply.turns[0] else {
        panic!("expected malformed edit_files call");
    };
    assert_eq!(name, "edit_files");
    assert!(args.pointer("/files/0/edits/0/oldText").is_none());
    assert_eq!(args["files"][0]["edits"][0][UNKNOWN_KEY], true);
    assert_eq!(args["files"][0]["edits"][0]["newText"], REPLACEMENT_VALUE);
    let repaired = request(SAFE_FEEDBACK, "{}", 9);
    let reply = super::reply(repaired.view.as_ref().unwrap());
    let Turn::ToolCall { id, name, args } = &reply.turns[0] else {
        panic!("expected the existing unread-companion batch");
    };
    assert_eq!(id, "batch-unread-companion-denied");
    assert_eq!(name, "edit_files");
    assert_eq!(args["files"].as_array().unwrap().len(), 2);
    assert_eq!(
        args["files"][0]["edits"][0]["oldText"],
        crate::live_manifest::batched_edits::PRIMARY_BEFORE
    );
}

#[test]
fn schema_feedback_bundle_preserves_graph_and_admission_contracts() {
    use crate::live_manifest::ScenarioBundle;
    let scenarios = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let baseline = ScenarioBundle::load(scenarios.join("mapped-live-batched-edits")).unwrap();
    let bundle =
        ScenarioBundle::load(scenarios.join("mapped-live-schema-rejection-feedback")).unwrap();
    assert_eq!(bundle.repo.seed_path, baseline.repo.seed_path);
    assert_eq!(
        bundle.resolved_manifest["expect"]["count"],
        baseline.resolved_manifest["expect"]["count"]
    );
    assert_eq!(
        bundle.resolved_manifest["validation"]["feature"].as_str(),
        Some("ai/temper#1322")
    );
    let sequence = bundle.resolved_manifest["expect"]["sequence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"].as_str() == Some("schema-feedback-before-admitted-batch"))
        .unwrap();
    let batch = sequence["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event
                .get("fields")
                .and_then(|f| f.get("tool"))
                .and_then(|v| v.as_str())
                == Some("edit_files")
        })
        .collect::<Vec<_>>();
    assert_eq!(batch.len(), 3);
    assert_eq!(
        batch[0]["fields"]["tool.failure.category"].as_str(),
        Some("schema_argument_mismatch")
    );
    assert_eq!(
        batch[1]["fields"]["tool.failure.category"].as_str(),
        Some("policy_denial")
    );
    assert_eq!(batch[2]["event"].as_str(), Some("tool.end"));
    assert_eq!(
        bundle.resolved_manifest["assertions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
