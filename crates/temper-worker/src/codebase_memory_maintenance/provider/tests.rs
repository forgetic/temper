// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn provider_record_reader_rejects_overflow_before_retaining_it() {
    let bytes = vec![b'x'; MAX_PROVIDER_RECORD_BYTES + 1];
    let mut reader = BufReader::new(std::io::Cursor::new(bytes));
    let error = read_bounded_record(&mut reader).expect_err("overflow fails closed");
    assert!(error.contains("byte bound"));
}

#[test]
fn inventory_parser_preserves_complete_ownership_and_lifecycle_metadata() {
    let page = parse_inventory_page(&json!({
        "cacheInstanceId": "cache-a",
        "cacheBytes": 8192,
        "projects": [{
            "name": "/workspace/engineer/key/temper",
            "metadata": {
                "repoPath": "/workspace/engineer/key/temper",
                "updatedAt": "2026-01-02T03:04:05Z",
                "managedBy": "temper",
                "estimatedBytes": 4096
            }
        }],
        "nextCursor": "page-2"
    }))
    .expect("page parses");
    assert_eq!(page.cache_instance_id.as_deref(), Some("cache-a"));
    assert_eq!(page.cache_bytes, Some(8192));
    assert_eq!(page.next_cursor.as_deref(), Some("page-2"));
    assert_eq!(page.projects[0].estimated_bytes, Some(4096));
    assert_eq!(page.projects[0].ownership.as_deref(), Some("temper"));
    assert!(page.projects[0].updated_at_unix_secs.is_some());
}

#[test]
fn maintenance_negotiation_requires_bounded_pagination_and_delete_identity() {
    let descriptors = BTreeMap::from([
        (
            "list_projects".to_string(),
            json!({"inputSchema": {
                "type": "object",
                "properties": {"limit": {"type": "integer"}, "cursor": {"type": "string"}},
                "required": ["limit"]
            }}),
        ),
        (
            "delete_project".to_string(),
            json!({"inputSchema": {
                "type": "object",
                "properties": {"project": {"type": "string"}},
                "required": ["project"]
            }}),
        ),
    ]);
    validate_maintenance_tools(&descriptors).expect("safe contract negotiates");

    let mut unbounded = descriptors.clone();
    unbounded
        .get_mut("list_projects")
        .unwrap()
        .get_mut("inputSchema")
        .unwrap()
        .get_mut("properties")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("limit");
    assert!(validate_maintenance_tools(&unbounded).is_err());
}

#[test]
fn recovery_negotiation_requires_target_status_safe_probe_and_stable_name() {
    let tools = BTreeMap::from([
        (
            "index_status".to_string(),
            json!({"inputSchema": {
                "properties": {"project": {"type": "string"}},
                "required": ["project"]
            }}),
        ),
        (
            "search_code".to_string(),
            json!({"inputSchema": {
                "properties": {
                    "project": {"type": "string"},
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }}),
        ),
        (
            "index_repository".to_string(),
            json!({"inputSchema": {
                "properties": {
                    "repo_path": {"type": "string"},
                    "name": {"type": "string"}
                },
                "required": ["repo_path"]
            }}),
        ),
    ]);
    validate_recovery_tools(&tools, true).expect("target recovery contract negotiates");

    let mut without_name = tools;
    without_name.get_mut("index_repository").unwrap()["inputSchema"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("name");
    assert!(validate_recovery_tools(&without_name, true).is_err());
}

#[test]
fn inventory_parser_retains_byte_and_active_indexing_evidence() {
    let page = parse_inventory_page(&json!({
        "cache_instance_id": "cache-a",
        "cache_bytes": 55_000,
        "projects": [{
            "project": "old",
            "repo_path": "/workspace/engineer/old/temper",
            "updated_at_unix_secs": 1,
            "estimated_bytes": 12_345,
            "status": "indexing"
        }]
    }))
    .expect("inventory parses");
    assert_eq!(page.cache_bytes, Some(55_000));
    assert_eq!(page.projects[0].estimated_bytes, Some(12_345));
    assert_eq!(page.projects[0].indexing_active, Some(true));

    let status = parse_project_status(
        &json!({"project": "temper-v1-key", "status": "ready"}),
        "temper-v1-key",
    )
    .expect("status parses");
    assert!(status.ready);
    assert!(!status.active);
    let active = parse_project_status(
        &json!({"project": "temper-v1-key", "status": "indexing"}),
        "temper-v1-key",
    )
    .expect("active status parses");
    assert!(active.active);
    assert!(!active.ready);
    assert!(
        parse_project_status(&json!({"status": "ready"}), "temper-v1-key").is_err(),
        "operator verification requires the provider to echo the exact identity"
    );
    assert!(
        parse_project_status(
            &json!({"project": "other", "status": "ready"}),
            "temper-v1-key"
        )
        .is_err()
    );

    let oversized = "s".repeat(MAX_PROVIDER_FIELD_BYTES + 1);
    let error = parse_inventory_page(&json!({
        "cache_instance_id": "cache-a",
        "projects": [{
            "project": oversized,
            "repo_path": "/workspace/engineer/old/temper",
            "updated_at_unix_secs": 1
        }]
    }))
    .expect_err("oversized untrusted identity is rejected");
    assert_eq!(error, "provider project identity exceeded its byte bound");
}

#[test]
fn provider_error_payloads_are_not_copied_into_operator_diagnostics() {
    let error = parse_tool_result(json!({
        "isError": true,
        "content": [{
            "type": "text",
            "text": "credential=must-not-leak"
        }]
    }))
    .expect_err("provider error is surfaced");
    assert_eq!(error, "provider tool returned an error response");
    assert!(!error.contains("must-not-leak"));

    let status_error = parse_project_status(
        &json!({"project": "expected", "status": "credential=must-not-leak"}),
        "expected",
    )
    .expect_err("unknown status is rejected");
    assert_eq!(
        status_error,
        "provider returned an unsupported index status"
    );
    assert!(!status_error.contains("must-not-leak"));

    let not_found = parse_tool_result(json!({
        "isError": true,
        "structuredContent": {
            "status": "not_found",
            "message": "project not found: sensitive/provider/path"
        }
    }))
    .expect_err("not-found is typed for idempotent deletion");
    assert_eq!(not_found, PROVIDER_PROJECT_NOT_FOUND);
    assert!(!not_found.contains("sensitive"));
}
