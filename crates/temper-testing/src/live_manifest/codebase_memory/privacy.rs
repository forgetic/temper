//! Privacy-safe retained evidence for mapped graph profiles.
//!
//! Raw MCP arguments and provider values remain in the temporary validator log.
//! Only this closed tool/checkpoint ordering is returned to scenario reporters.

use std::fs;
use std::path::PathBuf;

use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn is_privacy_safe_profile(profile: Option<&str>) -> bool {
    matches!(
        profile,
        Some(
            "provider-result-anchor"
                | "provider-neutral-anchor-lineage"
                | "mapped-live-graph-consumption"
                | "mapped-live-denied-shell-classification"
                | "mapped-live-ordinary-tool-convergence"
                | "mapped-live-graph-convergence"
                | "mapped-live-decision-gap-recovery"
                | "mapped-live-exact-source-selection"
                | "mapped-live-focused-test-source-relevance"
        )
    )
}

pub(super) fn write_privacy_safe_mcp_log(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Result<PathBuf, String> {
    let path = mcp
        .log_path
        .with_file_name("fake-codebase-memory-aggregate.jsonl");
    let mut safe = String::new();
    for (sequence, call) in calls.iter().enumerate() {
        safe.push_str(
            &serde_json::json!({
                "sequence": sequence + 1,
                "tool": call.name,
                "is_error": call.is_error,
                "checkpoint": call.fixture_event,
            })
            .to_string(),
        );
        safe.push('\n');
    }
    fs::write(&path, safe).map_err(|error| {
        format!(
            "write privacy-safe MCP evidence {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn exact_selection_aggregate_retains_only_closed_checkpoint_facts() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mcp = super::super::write_fake_mcp(
            workspace.path(),
            "private-provider-project",
            Some("mapped-live-exact-source-selection"),
            &["get_code_snippet".to_string()],
            &["index_repository".to_string()],
            750,
            None,
        )
        .expect("fake MCP");
        let calls = vec![McpToolCallEvidence {
            name: "get_code_snippet".to_string(),
            arguments: json!({
                "qualified_name": "private::implementation",
                "project": "private-provider-project",
                "source": "private source",
                "root": "/private/root",
                "credential": "MCP-FIXTURE-SECRET",
            }),
            delay_ms: None,
            is_error: false,
            fixture_event: Some("served_selection_implementation_source".to_string()),
        }];

        let path = write_privacy_safe_mcp_log(&mcp, &calls).expect("privacy-safe log");
        let retained = fs::read_to_string(path).expect("retained aggregate");
        assert_eq!(
            retained,
            concat!(
                "{\"checkpoint\":\"served_selection_implementation_source\",",
                "\"is_error\":false,\"sequence\":1,",
                "\"tool\":\"get_code_snippet\"}\n"
            )
        );
        for private in [
            "private::implementation",
            "private-provider-project",
            "private source",
            "/private/root",
            "credential",
            "MCP-FIXTURE-SECRET",
        ] {
            assert!(!retained.contains(private), "aggregate retained {private}");
        }
    }

    #[test]
    fn focused_relevance_aggregate_omits_selectors_source_and_credentials() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mcp = super::super::write_fake_mcp(
            workspace.path(),
            "private-provider-project",
            Some("mapped-live-focused-test-source-relevance"),
            &["search_graph".to_string()],
            &["index_repository".to_string()],
            750,
            None,
        )
        .expect("fake MCP");
        let calls = vec![McpToolCallEvidence {
            name: "search_graph".to_string(),
            arguments: json!({
                "query": "private semantic query",
                "selector": "private::focused_test",
                "credential": "MCP-FIXTURE-SECRET",
            }),
            delay_ms: None,
            is_error: false,
            fixture_event: Some("served_focus_fallback".to_string()),
        }];

        let path = write_privacy_safe_mcp_log(&mcp, &calls).expect("privacy-safe log");
        let retained = fs::read_to_string(path).expect("retained aggregate");
        assert_eq!(
            retained,
            concat!(
                "{\"checkpoint\":\"served_focus_fallback\",",
                "\"is_error\":false,\"sequence\":1,",
                "\"tool\":\"search_graph\"}\n"
            )
        );
        for private in [
            "private semantic query",
            "private::focused_test",
            "selector",
            "credential",
            "MCP-FIXTURE-SECRET",
        ] {
            assert!(!retained.contains(private), "aggregate retained {private}");
        }
    }

    #[test]
    fn decision_gap_aggregate_omits_private_recovery_data() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mcp = super::super::write_fake_mcp(
            workspace.path(),
            "private-provider-project",
            Some("mapped-live-decision-gap-recovery"),
            &["search_graph".to_string()],
            &["index_repository".to_string()],
            750,
            None,
        )
        .expect("fake MCP");
        let calls = vec![McpToolCallEvidence {
            name: "get_code_snippet".to_string(),
            arguments: json!({
                "qualified_name": "private::selector",
                "project": "private-provider-project",
                "credential": "MCP-FIXTURE-SECRET",
            }),
            delay_ms: None,
            is_error: false,
            fixture_event: Some("served_gap_active_source".to_string()),
        }];

        let path = write_privacy_safe_mcp_log(&mcp, &calls).expect("privacy-safe log");
        let retained = fs::read_to_string(path).expect("retained aggregate");
        assert_eq!(
            retained,
            concat!(
                "{\"checkpoint\":\"served_gap_active_source\",",
                "\"is_error\":false,\"sequence\":1,",
                "\"tool\":\"get_code_snippet\"}\n"
            )
        );
        for private in [
            "private::selector",
            "private-provider-project",
            "credential",
            "MCP-FIXTURE-SECRET",
        ] {
            assert!(!retained.contains(private), "aggregate retained {private}");
        }
    }
}
