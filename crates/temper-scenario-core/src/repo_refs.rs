// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use toml::Value;

use crate::toml_helpers::{join_field, string_value};
use crate::{Diagnostic, RepositoryReference};

pub(crate) fn collect_repository_references(
    value: &Value,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<RepositoryReference> {
    let mut repositories = Vec::new();
    collect_repositories_at(value, "", diagnostics, &mut repositories);
    repositories
}

fn collect_repositories_at(
    value: &Value,
    field_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
    repositories: &mut Vec<RepositoryReference>,
) {
    match value {
        Value::Table(table) => {
            for (key, child) in table {
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                let child_path = join_field(field_path, key);
                if matches!(normalized.as_str(), "repos" | "repositories") {
                    if looks_like_repository_declaration_collection(child) {
                        parse_repository_collection(child, &child_path, diagnostics, repositories);
                    }
                } else {
                    collect_repositories_at(child, &child_path, diagnostics, repositories);
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_repositories_at(
                    child,
                    &format!("{field_path}[{index}]"),
                    diagnostics,
                    repositories,
                );
            }
        }
        _ => {}
    }
}

fn parse_repository_collection(
    value: &Value,
    field_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
    repositories: &mut Vec<RepositoryReference>,
) {
    match value {
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                parse_repository_item(
                    child,
                    &format!("{field_path}[{index}]"),
                    None,
                    diagnostics,
                    repositories,
                );
            }
        }
        Value::Table(table) if looks_like_single_repository_table(table) => {
            parse_repository_table(table, field_path, None, diagnostics, repositories);
        }
        Value::Table(table) => {
            for (alias, child) in table {
                parse_repository_item(
                    child,
                    &join_field(field_path, alias),
                    Some(alias.as_str()),
                    diagnostics,
                    repositories,
                );
            }
        }
        _ => diagnostics.push(Diagnostic::error(
            field_path,
            "must be an array or table of repository references",
        )),
    }
}

fn parse_repository_item(
    value: &Value,
    field_path: &str,
    alias: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
    repositories: &mut Vec<RepositoryReference>,
) {
    match value {
        Value::String(repo) => {
            let field = field_path.to_string();
            if validate_repository_name(repo.trim(), &field, diagnostics) {
                repositories.push(RepositoryReference {
                    id: alias.map(ToOwned::to_owned),
                    repo: repo.trim().to_string(),
                    path: None,
                });
            }
        }
        Value::Table(table) => {
            parse_repository_table(table, field_path, alias, diagnostics, repositories);
        }
        _ => diagnostics.push(Diagnostic::error(
            field_path,
            "must be a repository string or table",
        )),
    }
}

fn parse_repository_table(
    table: &toml::Table,
    field_path: &str,
    alias: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
    repositories: &mut Vec<RepositoryReference>,
) {
    let id = table
        .get("id")
        .and_then(|value| string_value(join_field(field_path, "id"), value, diagnostics))
        .or_else(|| alias.map(ToOwned::to_owned));
    if let Some(id) = &id {
        let id_field = if table.contains_key("id") {
            join_field(field_path, "id")
        } else {
            field_path.to_string()
        };
        validate_alias(id, &id_field, diagnostics);
    }

    let repo_field = table
        .get("repo")
        .map(|value| ("repo", value))
        .or_else(|| table.get("repository").map(|value| ("repository", value)))
        .or_else(|| table.get("slug").map(|value| ("slug", value)))
        .or_else(|| table.get("remote").map(|value| ("remote", value)))
        .or_else(|| {
            table.get("name").and_then(|value| {
                value
                    .as_str()
                    .is_some_and(|name| name.contains('/'))
                    .then_some(("name", value))
            })
        });
    let Some((repo_key, repo_value)) = repo_field else {
        diagnostics.push(Diagnostic::error(
            join_field(field_path, "repo"),
            "repository reference is required",
        ));
        return;
    };
    let repo_field_path = join_field(field_path, repo_key);
    let Some(repo) = string_value(repo_field_path.clone(), repo_value, diagnostics) else {
        return;
    };
    if !validate_repository_name(&repo, &repo_field_path, diagnostics) {
        return;
    }

    let path = table
        .get("path")
        .and_then(|value| string_value(join_field(field_path, "path"), value, diagnostics));

    repositories.push(RepositoryReference { id, repo, path });
}

fn looks_like_single_repository_table(table: &toml::Table) -> bool {
    ["repo", "repository", "remote", "name", "id", "slug", "path"]
        .iter()
        .any(|key| table.contains_key(*key))
}

fn looks_like_repository_declaration_collection(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(looks_like_repository_declaration_item),
        Value::Table(table) if looks_like_single_repository_table(table) => true,
        Value::Table(table) => table.values().any(looks_like_repository_declaration_item),
        _ => false,
    }
}

fn looks_like_repository_declaration_item(value: &Value) -> bool {
    match value {
        Value::String(repo) => repo.trim().contains('/'),
        Value::Table(table) => looks_like_single_repository_table(table),
        _ => false,
    }
}

pub(crate) fn repository_aliases(repositories: &[RepositoryReference]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::new();
    for repository in repositories {
        aliases.insert(repository.repo.clone());
        if let Some(id) = &repository.id {
            aliases.insert(id.clone());
        }
    }
    aliases
}

pub(crate) fn validate_repository_fields(
    value: &Value,
    field_path: &str,
    aliases: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        Value::Table(table) => {
            for (key, child) in table {
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                let child_path = join_field(field_path, key);
                if matches!(normalized.as_str(), "repos" | "repositories") {
                    if !looks_like_repository_declaration_collection(child) {
                        validate_repository_reference_value(
                            child,
                            &child_path,
                            aliases,
                            diagnostics,
                        );
                    }
                    continue;
                }
                if is_repository_field_key(&normalized) {
                    validate_repository_reference_value(child, &child_path, aliases, diagnostics);
                } else {
                    validate_repository_fields(child, &child_path, aliases, diagnostics);
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                validate_repository_fields(
                    child,
                    &format!("{field_path}[{index}]"),
                    aliases,
                    diagnostics,
                );
            }
        }
        _ => {}
    }
}

fn validate_repository_reference_value(
    value: &Value,
    field_path: &str,
    aliases: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        Value::String(repo) => {
            let repo = repo.trim();
            if !aliases.contains(repo) {
                validate_repository_name(repo, field_path, diagnostics);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                validate_repository_reference_value(
                    child,
                    &format!("{field_path}[{index}]"),
                    aliases,
                    diagnostics,
                );
            }
        }
        Value::Table(_) => validate_repository_fields(value, field_path, aliases, diagnostics),
        _ => diagnostics.push(Diagnostic::error(
            field_path,
            "must be a repository string in `owner/name` form or a declared repository id",
        )),
    }
}

fn is_repository_field_key(key: &str) -> bool {
    matches!(
        key,
        "repo" | "repository" | "target_repo" | "target_repository"
    )
}

pub(crate) fn validate_repository_name(
    repo: &str,
    field_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let parts: Vec<_> = repo.split('/').collect();
    let valid = parts.len() == 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(is_repo_component_char));
    if !valid {
        diagnostics.push(Diagnostic::error(
            field_path,
            "repository must be in `owner/name` form using letters, digits, `.`, `_`, or `-`",
        ));
    }
    valid
}

fn validate_alias(alias: &str, field_path: &str, diagnostics: &mut Vec<Diagnostic>) -> bool {
    let valid = !alias.is_empty() && alias.chars().all(is_repo_component_char);
    if !valid {
        diagnostics.push(Diagnostic::error(
            field_path,
            "repository id must use letters, digits, `.`, `_`, or `-`",
        ));
    }
    valid
}

fn is_repo_component_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}
