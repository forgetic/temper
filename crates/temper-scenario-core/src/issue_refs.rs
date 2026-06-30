// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use toml::Value;

use crate::repo_refs::validate_repository_name;
use crate::toml_helpers::{join_field, string_value};
use crate::{Diagnostic, IssueReference};

pub(crate) fn collect_issue_references(
    value: &Value,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<IssueReference> {
    let mut issues = Vec::new();
    collect_issues_at(
        value,
        "",
        None,
        aliases,
        repository_count,
        diagnostics,
        &mut issues,
    );
    issues
}

fn collect_issues_at(
    value: &Value,
    field_path: &str,
    inherited_repo: Option<&str>,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
    issues: &mut Vec<IssueReference>,
) {
    match value {
        Value::Table(table) => {
            let local_repo = table
                .get("repo")
                .or_else(|| table.get("repository"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|repo| !repo.is_empty())
                .or(inherited_repo);
            for (key, child) in table {
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                let child_path = join_field(field_path, key);
                if is_issue_field_key(&normalized) {
                    parse_issue_value(
                        child,
                        &child_path,
                        local_repo,
                        aliases,
                        repository_count,
                        diagnostics,
                        issues,
                    );
                } else if matches!(normalized.as_str(), "repos" | "repositories") {
                    continue;
                } else {
                    collect_issues_at(
                        child,
                        &child_path,
                        local_repo,
                        aliases,
                        repository_count,
                        diagnostics,
                        issues,
                    );
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_issues_at(
                    child,
                    &format!("{field_path}[{index}]"),
                    inherited_repo,
                    aliases,
                    repository_count,
                    diagnostics,
                    issues,
                );
            }
        }
        _ => {}
    }
}

fn parse_issue_value(
    value: &Value,
    field_path: &str,
    inherited_repo: Option<&str>,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
    issues: &mut Vec<IssueReference>,
) {
    match value {
        Value::Integer(number) => push_numeric_issue(
            *number,
            inherited_repo,
            field_path,
            aliases,
            repository_count,
            diagnostics,
            issues,
        ),
        Value::String(reference) => parse_issue_string(
            reference,
            field_path,
            inherited_repo,
            aliases,
            repository_count,
            diagnostics,
            issues,
        ),
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                parse_issue_value(
                    child,
                    &format!("{field_path}[{index}]"),
                    inherited_repo,
                    aliases,
                    repository_count,
                    diagnostics,
                    issues,
                );
            }
        }
        Value::Table(table) => parse_issue_table(
            table,
            field_path,
            inherited_repo,
            aliases,
            repository_count,
            diagnostics,
            issues,
        ),
        _ => diagnostics.push(Diagnostic::error(
            field_path,
            "must be an issue number, `#number`, `owner/repo#number`, or table",
        )),
    }
}

fn parse_issue_table(
    table: &toml::Table,
    field_path: &str,
    inherited_repo: Option<&str>,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
    issues: &mut Vec<IssueReference>,
) {
    let repo = table
        .get("repo")
        .or_else(|| table.get("repository"))
        .and_then(|value| string_value(join_field(field_path, "repo"), value, diagnostics))
        .or_else(|| inherited_repo.map(ToOwned::to_owned));
    let number_value = table
        .get("number")
        .or_else(|| table.get("issue"))
        .or_else(|| table.get("id"));
    let Some(number_value) = number_value else {
        diagnostics.push(Diagnostic::error(
            join_field(field_path, "number"),
            "issue number is required",
        ));
        return;
    };

    match number_value {
        Value::Integer(number) => push_numeric_issue(
            *number,
            repo.as_deref(),
            &join_field(field_path, "number"),
            aliases,
            repository_count,
            diagnostics,
            issues,
        ),
        Value::String(reference) => parse_issue_string(
            reference,
            &join_field(field_path, "number"),
            repo.as_deref(),
            aliases,
            repository_count,
            diagnostics,
            issues,
        ),
        _ => diagnostics.push(Diagnostic::error(
            join_field(field_path, "number"),
            "must be an issue number or issue reference string",
        )),
    }
}

fn parse_issue_string(
    reference: &str,
    field_path: &str,
    inherited_repo: Option<&str>,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
    issues: &mut Vec<IssueReference>,
) {
    let reference = reference.trim();
    if reference.is_empty() {
        diagnostics.push(Diagnostic::error(
            field_path,
            "issue reference must not be empty",
        ));
        return;
    }

    if let Some(number) = reference.strip_prefix('#') {
        if let Some(number) = parse_issue_number(number, field_path, diagnostics) {
            push_issue(
                inherited_repo.map(ToOwned::to_owned),
                number,
                field_path,
                aliases,
                repository_count,
                diagnostics,
                issues,
            );
        }
        return;
    }

    if let Some((repo, number)) = reference.rsplit_once('#') {
        if let Some(number) = parse_issue_number(number, field_path, diagnostics) {
            push_issue(
                Some(repo.trim().to_string()),
                number,
                field_path,
                aliases,
                repository_count,
                diagnostics,
                issues,
            );
        }
        return;
    }

    if reference.chars().all(|ch| ch.is_ascii_digit()) {
        if let Some(number) = parse_issue_number(reference, field_path, diagnostics) {
            push_issue(
                inherited_repo.map(ToOwned::to_owned),
                number,
                field_path,
                aliases,
                repository_count,
                diagnostics,
                issues,
            );
        }
        return;
    }

    diagnostics.push(Diagnostic::error(
        field_path,
        "must be an issue number, `#number`, or `owner/repo#number`",
    ));
}

fn push_numeric_issue(
    number: i64,
    repo: Option<&str>,
    field_path: &str,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
    issues: &mut Vec<IssueReference>,
) {
    if number <= 0 {
        diagnostics.push(Diagnostic::error(
            field_path,
            "issue number must be positive",
        ));
        return;
    }
    push_issue(
        repo.map(ToOwned::to_owned),
        number as u64,
        field_path,
        aliases,
        repository_count,
        diagnostics,
        issues,
    );
}

fn parse_issue_number(
    number: &str,
    field_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u64> {
    match number.trim().parse::<u64>() {
        Ok(0) => {
            diagnostics.push(Diagnostic::error(
                field_path,
                "issue number must be positive",
            ));
            None
        }
        Ok(number) => Some(number),
        Err(_) => {
            diagnostics.push(Diagnostic::error(
                field_path,
                "issue number must be positive",
            ));
            None
        }
    }
}

fn push_issue(
    repo: Option<String>,
    number: u64,
    field_path: &str,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
    issues: &mut Vec<IssueReference>,
) {
    if !validate_issue_repo(
        repo.as_deref(),
        field_path,
        aliases,
        repository_count,
        diagnostics,
    ) {
        return;
    }
    issues.push(IssueReference { repo, number });
}

fn validate_issue_repo(
    repo: Option<&str>,
    field_path: &str,
    aliases: &BTreeSet<String>,
    repository_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(repo) = repo else {
        if repository_count == 1 {
            return true;
        }
        diagnostics.push(Diagnostic::error(
            field_path,
            if repository_count == 0 {
                "issue reference must include a repository"
            } else {
                "issue reference must include a repository when multiple repositories are declared"
            },
        ));
        return false;
    };
    let repo = repo.trim();
    if aliases.contains(repo) {
        return true;
    }
    validate_repository_name(repo, field_path, diagnostics)
}

fn is_issue_field_key(key: &str) -> bool {
    matches!(key, "issue" | "issues" | "issue_ref" | "issue_refs")
        || key.ends_with("_issue")
        || key.ends_with("_issues")
}
