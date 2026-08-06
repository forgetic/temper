use std::path::Path;

use serde_json::Value as JsonValue;

pub(super) fn seed(root: &Path) -> JsonValue {
    let projects = (0..320)
        .map(|index| {
            serde_json::json!({
                "project": format!("legacy-temper-{index:03}"),
                "repo_path": root.join("workspaces/engineer").join(format!("old-{index:03}/demo")).display().to_string(),
                "status": "stale",
                "updated_at_unix_secs": 1,
                "ownership": "temper",
                "estimated_bytes": 4096
            })
        })
        .chain([
            serde_json::json!({"project":"temper-v1-protected","repo_path":root.join("workspaces/engineer/stable/demo").display().to_string(),"status":"fresh","ownership":"temper","estimated_bytes":8192}),
            serde_json::json!({"project":"unrelated-project","repo_path":"/opt/unrelated/repo","status":"stale","estimated_bytes":2048}),
            serde_json::json!({"project":"ambiguous-project","repo_path":root.join("workspaces/engineer/ambiguous").display().to_string(),"status":"stale","estimated_bytes":1024}),
            serde_json::json!({"project":"active-project","repo_path":root.join("workspaces/engineer/active/demo").display().to_string(),"status":"stale","ownership":"temper","estimated_bytes":4096}),
        ])
        .collect::<Vec<_>>();
    serde_json::json!({
        "cache_instance_id": "scenario-lifecycle-cache-v1",
        "cache_bytes": 1_327_104,
        "now_unix_secs": 2_000_000_000u64,
        "projects": projects
    })
}
