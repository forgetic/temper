// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use crate::Diagnostic;

pub(crate) fn join_field(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

pub(crate) fn string_value(
    field: impl Into<String>,
    value: &Value,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let field = field.into();
    let Some(raw) = value.as_str() else {
        diagnostics.push(Diagnostic::error(field, "must be a string"));
        return None;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        diagnostics.push(Diagnostic::error(field, "must not be empty"));
        return None;
    }
    Some(trimmed.to_string())
}
