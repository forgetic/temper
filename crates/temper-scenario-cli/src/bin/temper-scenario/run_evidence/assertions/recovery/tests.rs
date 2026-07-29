// SPDX-License-Identifier: MPL-2.0

use serde_json::json;

use super::*;

#[test]
fn structured_recovery_assertions_pass_on_complete_bounded_evidence() {
    let artifact = artifact();
    let expect = expectations();
    let mut results = Vec::new();

    evaluate(&expect, &artifact, &mut results);

    assert_eq!(results.len(), 5);
    assert!(
        results.iter().all(|result| result.status == "passed"),
        "{results:#?}"
    );
}

#[test]
fn duplicate_requests_tools_publications_and_premature_landing_block_validation() {
    let mut artifact = artifact();
    artifact.provider.as_mut().unwrap().request_ids[2] = "engineer:2".to_string();
    let observability = artifact.observability.as_mut().unwrap();
    observability.events.insert(
        3,
        serde_json::from_value(json!({
            "sequence": 4,
            "event": "pr.opened",
            "fields": {}
        }))
        .unwrap(),
    );
    observability.events.push(
        serde_json::from_value(json!({
            "sequence": 6,
            "event": "tool.start",
            "fields": {"tool": "write"}
        }))
        .unwrap(),
    );
    let mut duplicate_pr = artifact.final_state.pull_requests[0].clone();
    duplicate_pr.number = 2;
    artifact.final_state.pull_requests.push(duplicate_pr);
    let mut results = Vec::new();

    evaluate(&expectations(), &artifact, &mut results);

    for id in [
        "provider-budget",
        "workspace-retained",
        "publication-fenced",
    ] {
        let result = results.iter().find(|result| result.id == id).unwrap();
        assert_eq!(result.status, "failed", "{result:#?}");
    }
}

#[test]
fn unsupported_required_recovery_fields_remain_visible_and_blocking() {
    let value: Value = toml::from_str(
        r#"
            [[recovery]]
            id = "unsupported-prose"
            event = "model.provider.deferred"
            provider_prose = "must never be accepted"
        "#,
    )
    .unwrap();
    let mut results = Vec::new();

    evaluate(value.as_table().unwrap(), &artifact(), &mut results);

    assert_eq!(results.len(), 1);
    assert!(results[0].required);
    assert_eq!(results[0].status, "unsupported");
    assert!(results[0].details[0].contains("provider_prose"));
}

fn expectations() -> toml::Table {
    let value: Value = toml::from_str(
        r#"
            [[provider_requests]]
            id = "provider-budget"
            role = "engineer"
            exactly = 3
            max = 3
            unique = true

            [[recovery]]
            id = "deferred-state"
            event = "model.provider.deferred"
            action = "provider_deferred"
            disposition = "unknown"
            boundary = "sse"
            event_kind = "stream_error"
            session_number = 2
            session_failure_count = 1
            cumulative_failure_count = 3
            elapsed_ms = 45000
            deferral_count = 1
            generation = 1
            status_present = false
            code_present = false

            [[stimuli]]
            id = "wake-stimulus"
            stimulus = "wake-provider"
            action = "provider.health_wake"
            status = "passed"
            attempts = 1
            details_contain = "authenticated"

            [[workspace]]
            id = "workspace-retained"
            retained = true
            path_contains = "coordination-workspace"
            tool = "write"
            tool_effects = 1
            max_tool_effects = 1

            [[publication]]
            id = "publication-fenced"
            pull_requests = 1
            branches = 2
            blocked_while_deferred = true
        "#,
    )
    .unwrap();
    value.as_table().unwrap().clone()
}

fn artifact() -> RunEvidenceArtifact {
    serde_json::from_value(json!({
        "schema": "temper.scenario.run-evidence",
        "version": 2,
        "scenario": {
            "name": "recovery-fixture",
            "source": "checked-in",
            "source_description": "fixture",
            "scenario_path": "scenarios/recovery-fixture",
            "manifest_path": "scenarios/recovery-fixture/scenario.toml",
            "runner_id": "manifest",
            "runner_selector": "runner.uses",
            "runner_selection": "manifest",
            "tier": "live",
            "tier_description": "real stack",
            "topology": {}
        },
        "final_state": {
            "pull_requests": [{
                "number": 1,
                "state": "merged",
                "head_branch": "agent/recovered"
            }],
            "repositories": [{
                "id": "service",
                "branches": [{"name": "main"}, {"name": "agent/recovered"}]
            }],
            "ci": {}
        },
        "provider": {
            "request_count": 3,
            "request_ids": ["engineer:1", "engineer:2", "engineer:3"],
            "request_counts_by_role": {"engineer": 3}
        },
        "observability": {
            "scenario_run_id": "run-1",
            "log_format": "json",
            "rust_log": "temper=debug",
            "event_log_path": "standalone.log",
            "event_log_paths": ["standalone.log"],
            "captured_events": 4,
            "events": [
                {
                    "sequence": 1,
                    "event": "model.turn.retrying",
                    "fields": {"attempt": "0", "next_attempt": "1"}
                },
                {
                    "sequence": 2,
                    "event": "model.provider.deferred",
                    "fields": {
                        "action": "provider_deferred",
                        "disposition": "unknown",
                        "boundary": "sse",
                        "event_kind": "stream_error",
                        "session_number": "2",
                        "session_failure_count": "1",
                        "cumulative_failure_count": "3",
                        "elapsed_ms": "45000",
                        "deferral_count": "1",
                        "generation": "1",
                        "status_present": "false",
                        "code_present": "false"
                    }
                },
                {
                    "sequence": 3,
                    "event": "tool.start",
                    "fields": {"tool": "write"}
                },
                {
                    "sequence": 5,
                    "event": "model.provider.wake",
                    "fields": {"generation": "2"}
                }
            ]
        },
        "artifacts": {
            "artifact_paths": ["/tmp/coordination-workspace"]
        },
        "stimuli": [{
            "id": "wake-provider",
            "action": "provider.health_wake",
            "status": "passed",
            "attempts": 1,
            "timeout_ms": 30000,
            "duration_ms": 10,
            "details": ["authenticated wake advanced"]
        }]
    }))
    .unwrap()
}
