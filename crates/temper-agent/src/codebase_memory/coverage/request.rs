//! Bounded, canonical repository-relative coverage requests.
use serde_json::{Value, json};
use std::path::{Component, Path};

pub(super) const MAX_PATHS: usize = 16;
pub(super) const MAX_SCOPES: usize = 4;
pub(super) const MAX_ENTRIES: u64 = 32;

pub(in crate::codebase_memory) fn schema() -> Value {
    json!({"type":"object", "properties": {
        "project":{"type":"string"},
        "paths":{"type":"array","maxItems":MAX_PATHS,"items":{"type":"string","maxLength":512}},
        "scopes":{"type":"array","maxItems":MAX_SCOPES,"items":{"type":"string","maxLength":512}},
        "scope_limit":{"type":"integer","minimum":1,"maximum":MAX_ENTRIES,"default":16},
        "scope_offset":{"type":"integer","minimum":0,"maximum":4096,"default":0},
        "generation":{"type":"string","maxLength":128,"description":"Expected generation from previous coverage; required when continuing scope pagination."}
    },"additionalProperties":false})
}

pub(super) fn validate(input: &mut Value, root: &Path) -> Result<Option<String>, &'static str> {
    let object = input.as_object_mut().ok_or("coverage requires an object")?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "project" | "repo" | "paths" | "scopes" | "scope_limit" | "scope_offset" | "generation"
        )
    }) {
        return Err("unknown coverage argument");
    }
    let mut count = 0;
    for (key, max, directory) in [("paths", MAX_PATHS, false), ("scopes", MAX_SCOPES, true)] {
        if let Some(value) = object.get(key) {
            let values = value
                .as_array()
                .ok_or("coverage paths/scopes must be arrays")?;
            if values.len() > max {
                return Err("coverage batch exceeds bound");
            }
            let mut seen = std::collections::BTreeSet::new();
            for value in values {
                let value = value.as_str().ok_or("coverage path must be a string")?;
                safe_path(value, directory)?;
                if !seen.insert(value) {
                    return Err("duplicate coverage path");
                }
                let mut path = root.join(value);
                while !path.exists() {
                    if !path.pop() {
                        return Err("coverage path unavailable");
                    }
                }
                let canonical = path
                    .canonicalize()
                    .map_err(|_| "coverage path unavailable")?;
                if !canonical.starts_with(root) {
                    return Err("coverage path escapes checkout");
                }
            }
            count += values.len();
        }
    }
    if count == 0 {
        return Err("coverage requires paths or scopes");
    }
    for (key, default, min, max) in [
        ("scope_limit", 16, 1, MAX_ENTRIES),
        ("scope_offset", 0, 0, 4096),
    ] {
        let value = object
            .entry(key)
            .or_insert(json!(default))
            .as_u64()
            .ok_or("invalid coverage pagination")?;
        if value < min || value > max {
            return Err("coverage pagination exceeds bound");
        }
    }
    let generation = object
        .remove("generation")
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
                .map(str::to_string)
                .ok_or("invalid coverage generation")
        })
        .transpose()?;
    if object["scope_offset"] != 0 && generation.is_none() {
        return Err("pagination requires expected generation");
    }
    Ok(generation)
}

pub(super) fn safe_path(value: &str, directory: bool) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 512
        || value.contains('\\')
        || value.contains(':')
        || value.starts_with('~')
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .any(|p| p.is_empty() || p == ".." || (p == "." && !(directory && value == ".")))
        || Path::new(value)
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err("coverage path must be canonical repository-relative");
    }
    Ok(())
}
