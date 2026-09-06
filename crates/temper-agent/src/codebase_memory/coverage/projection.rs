//! Keep only coverage protocol fields; provider transcripts never enter handoffs.
use serde_json::{Map, Value};

pub(super) fn project(provider: &Value) -> Result<Value, &'static str> {
    let mut result = fields(provider, &["project", "signal", "indexed_at"])?;
    result["metadata"] = fields(
        &provider["metadata"],
        &[
            "generation",
            "index_mode",
            "recorded_at",
            "recording_status",
            "ignored_files_stored",
            "ignored_files_total",
            "hash_records_complete",
            "coverage_version",
            "generation_matches",
        ],
    )?;
    for (name, keys) in [
        (
            "paths",
            &[
                "requested_path",
                "path",
                "status",
                "freshness",
                "recommended_action",
            ][..],
        ),
        (
            "scopes",
            &[
                "requested_scope",
                "scope",
                "status",
                "total",
                "has_more",
                "nextCursor",
            ][..],
        ),
    ] {
        let mut records = Vec::new();
        for record in provider[name].as_array().into_iter().flatten() {
            let mut projected = fields(record, keys)?;
            let key = if name == "paths" {
                "coverage"
            } else {
                "entries"
            };
            let flags = record[key].as_array().ok_or("missing coverage flags")?;
            if flags.len() > 64 {
                return Err("too many coverage flags; narrow request");
            }
            projected[key] = Value::Array(
                flags
                    .iter()
                    .map(|flag| {
                        fields(
                            flag,
                            &[
                                "path",
                                "kind",
                                "detail",
                                "reason",
                                "line_start",
                                "line_end",
                                "start_line",
                                "end_line",
                                "ranges",
                                "line_ranges",
                            ],
                        )
                    })
                    .collect::<Result<_, _>>()?,
            );
            records.push(projected);
        }
        result[name] = Value::Array(records);
    }
    Ok(result)
}

fn fields(value: &Value, keys: &[&str]) -> Result<Value, &'static str> {
    let object = value.as_object().ok_or("malformed coverage record")?;
    let mut result = Map::new();
    for key in keys {
        if let Some(value) = object.get(*key) {
            validate_value(value, 0)?;
            result.insert((*key).to_string(), value.clone());
        }
    }
    Ok(Value::Object(result))
}
fn validate_value(value: &Value, depth: usize) -> Result<(), &'static str> {
    if depth > 3 {
        return Err("coverage flag nesting exceeds bound");
    }
    match value {
        Value::String(s) if s.len() > 512 || s.chars().any(char::is_control) => {
            Err("coverage field exceeds text bound")
        }
        Value::Array(values) if values.len() <= 64 => {
            for v in values {
                validate_value(v, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(object) if object.len() <= 8 => {
            for (key, v) in object {
                if !matches!(
                    key.as_str(),
                    "start"
                        | "end"
                        | "start_line"
                        | "end_line"
                        | "line_start"
                        | "line_end"
                        | "start_byte"
                        | "end_byte"
                ) {
                    return Err("unknown coverage range field");
                }
                validate_value(v, depth + 1)?;
            }
            Ok(())
        }
        Value::Array(_) | Value::Object(_) => Err("coverage flag exceeds bound"),
        _ => Ok(()),
    }
}
