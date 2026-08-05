// SPDX-License-Identifier: MPL-2.0

use super::*;

pub(super) fn parse(table: &toml::Table, index: usize) -> Result<ManifestAction, String> {
    let field = format!("steps[{index}]");
    let omit_webhooks = table
        .get("omit_webhooks")
        .and_then(TomlValue::as_bool)
        .ok_or_else(|| format!("{field}.omit_webhooks must be boolean true"))?;
    if !omit_webhooks {
        return Err(format!(
            "{field}.omit_webhooks must be true so targeted delivery cannot satisfy the recovery proof"
        ));
    }
    Ok(ManifestAction::SeedTerminalHistory {
        fixture: TerminalHistorySeedFixture {
            repo_id: required_table_string(table, "repo", &field)?,
            actionable_issue_id: required_table_string(table, "actionable_issue_id", &field)?,
            target_closed_issues: bounded_integer(table, "closed_issues", &field, 200, 1, 500)?
                as usize,
            target_closed_pull_requests: bounded_integer(
                table,
                "closed_pull_requests",
                &field,
                100,
                1,
                250,
            )? as usize,
            inert_issue_labels: required_string_array(table, "issue_labels", &field)?,
            inert_pull_request_labels: required_string_array(table, "pull_request_labels", &field)?,
            sibling_repo_slug: required_table_string(table, "sibling_repo", &field)?,
            sibling_closed_issues: bounded_integer(
                table,
                "sibling_closed_issues",
                &field,
                200,
                1,
                500,
            )? as usize,
            sibling_issue_labels: required_string_array(table, "sibling_issue_labels", &field)?,
            omit_webhooks,
        },
    })
}

fn required_string_array(
    table: &toml::Table,
    key: &str,
    field: &str,
) -> Result<Vec<String>, String> {
    let values = string_array(table, key, field)?;
    (!values.is_empty())
        .then_some(values)
        .ok_or_else(|| format!("{field}.{key} must contain at least one string"))
}
