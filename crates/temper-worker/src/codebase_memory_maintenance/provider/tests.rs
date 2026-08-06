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
