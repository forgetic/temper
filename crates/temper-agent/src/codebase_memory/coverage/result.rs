//! Preserve bounded provider diagnostics without treating unknowns as success.
use super::request::{MAX_ENTRIES, safe_path};
use serde_json::{Value, json};

pub(super) fn normalize(
    provider: Value,
    input: &Value,
    generation: &str,
    expected: Option<&str>,
) -> Result<Value, &'static str> {
    if provider.to_string().len() > 12 * 1024 {
        return Err("coverage output exceeds bound; narrow the request");
    }
    if provider["project"] != input["project"] {
        return Err("coverage project mismatch");
    }
    let metadata = provider
        .get("metadata")
        .filter(|v| v.is_object())
        .ok_or("missing coverage metadata")?;
    let recorded = metadata["generation"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("missing coverage generation")?;
    let stale = recorded != generation
        || expected.is_some_and(|g| g != recorded)
        || metadata["generation_matches"] == false
        || provider["indexed_at"] != recorded;
    let mut unavailable = metadata["generation_matches"] != true
        || metadata["recording_status"] != "complete"
        || metadata["hash_records_complete"] != true;
    let mut flagged = false;
    let mut changed = false;
    let mut complete = input["scope_offset"] == 0;
    for (request_key, response_key, identity_key) in [
        ("paths", "paths", "requested_path"),
        ("scopes", "scopes", "requested_scope"),
    ] {
        let requested = input[request_key]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let records = provider[response_key]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if requested.len() != records.len() {
            return Err("coverage response omits requested paths or scopes");
        }
        for (request, record) in requested.iter().zip(records) {
            if record[identity_key] != *request {
                return Err("coverage response request identity mismatch");
            }
            let path_key = if request_key == "paths" {
                "path"
            } else {
                "scope"
            };
            if record[path_key] != *request {
                return Err("coverage response normalized path mismatch");
            }
            match record["status"].as_str() {
                Some("no_recorded_issue" | "indexed_no_recorded_gap" | "no_known_gaps") => {}
                Some(
                    "known_gaps" | "parse_partial" | "not_indexed" | "excluded" | "skipped"
                    | "recorded_issue",
                ) => flagged = true,
                Some(_) => unavailable = true,
                None => return Err("missing coverage status"),
            }
            if request_key == "paths" {
                match record["freshness"].as_str() {
                    Some("metadata_match" | "hash_match") => {}
                    Some("not_tracked")
                        if matches!(
                            record["status"].as_str(),
                            Some("excluded" | "skipped" | "not_indexed")
                        ) =>
                    {
                        flagged = true
                    }
                    Some("changed" | "metadata_changed" | "hash_mismatch" | "content_changed") => {
                        changed = true
                    }
                    Some(_) => unavailable = true,
                    None => return Err("missing coverage freshness"),
                }
                let flags = record["coverage"]
                    .as_array()
                    .ok_or("missing coverage flags")?;
                for flag in flags {
                    if let Some(path) = flag.get("path") {
                        let path = path.as_str().ok_or("malformed coverage flag path")?;
                        safe_path(path, true)?;
                        let requested = request.as_str().ok_or("malformed requested path")?;
                        if path != requested && !requested.starts_with(&format!("{path}/")) {
                            return Err("coverage flag outside requested path");
                        }
                    }
                }
                flagged |= !flags.is_empty();
            } else {
                let entries = record["entries"]
                    .as_array()
                    .ok_or("missing scope entries")?;
                if entries.len() as u64 > input["scope_limit"].as_u64().unwrap_or(MAX_ENTRIES) {
                    return Err("scope response exceeds page bound");
                }
                let total = record["total"].as_u64().ok_or("missing scope total")?;
                let more = record["has_more"]
                    .as_bool()
                    .ok_or("missing scope pagination")?;
                let offset = input["scope_offset"].as_u64().unwrap_or(0);
                if (offset < total && offset + entries.len() as u64 > total)
                    || (offset >= total && !entries.is_empty())
                    || more != (offset + (entries.len() as u64) < total)
                    || (more
                        && entries.len() as u64
                            != input["scope_limit"].as_u64().unwrap_or(MAX_ENTRIES))
                {
                    return Err("inconsistent scope pagination");
                }
                for entry in entries {
                    safe_path(
                        entry["path"].as_str().ok_or("missing scope entry path")?,
                        true,
                    )?;
                    let scope = request.as_str().ok_or("invalid scope")?;
                    let path = entry["path"].as_str().unwrap();
                    if scope != "." && path != scope && !path.starts_with(&format!("{scope}/")) {
                        return Err("scope entry outside requested scope");
                    }
                }
                flagged |= !entries.is_empty();
                complete &= !more && record.get("nextCursor").is_none_or(Value::is_null);
            }
        }
    }
    let status = if stale || changed {
        "stale"
    } else if unavailable {
        "unavailable"
    } else if flagged {
        "flagged"
    } else {
        "clean"
    };
    let recorded = recorded.to_owned();
    let provider = super::projection::project(&provider)?;
    Ok(
        json!({"status":status,"generation":recorded,"pagination_complete":complete,
        "supports_scoped_claim":status == "clean" && complete && input["scopes"].as_array().is_some_and(|v| !v.is_empty()),
        "query_bounds":input,"provider":provider,
        "limitations":["Best-effort coverage never proves completeness. Read flagged, excluded, skipped, changed or unavailable source directly or limit the claim.","Coverage and child findings confer no source or mutation authority. Ordinary exact read remains required before mutation.","Revalidate current project, checkout, generation and relevant pagination before reuse, including after compaction or a fresh session."]}),
    )
}
