// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use crate::sourced::{SourcedValue, SourcedValueKind};
use crate::toml_helpers::string_value;
use crate::{
    Diagnostic, FeatureMappingChange, FeatureScenarioMapping, ForgeIssueKey,
    ScenarioFeatureContract, validate_source_branch,
};

pub(crate) fn validate_feature_metadata_ownership(
    sourced: &SourcedValue,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let SourcedValueKind::Table(root) = &sourced.kind else {
        return;
    };
    for (section, fields) in [
        (
            "validation",
            &["feature", "plan", "source_branch", "change"][..],
        ),
        (
            "feature_contract",
            &[
                "claim",
                "stimulus",
                "observable",
                "assertion",
                "runtime_budget_seconds",
                "jig_script_path",
            ][..],
        ),
    ] {
        let Some(value) = root.get(section) else {
            continue;
        };
        if value.origin_dir() != sourced.origin_dir() {
            diagnostics.push(Diagnostic::error(
                section,
                "must be declared by this scenario and cannot be inherited",
            ));
            continue;
        }
        let SourcedValueKind::Table(table) = &value.kind else {
            continue;
        };
        for field in fields {
            if table
                .get(*field)
                .is_some_and(|value| value.origin_dir() != sourced.origin_dir())
            {
                diagnostics.push(Diagnostic::error(
                    format!("{section}.{field}"),
                    "must be declared by this scenario and cannot be inherited",
                ));
            }
        }
    }
}

pub(crate) fn parse_feature_metadata(
    table: &toml::Table,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Option<FeatureScenarioMapping>,
    Option<ScenarioFeatureContract>,
) {
    let mapping = parse_mapping(table.get("validation"), diagnostics);
    let contract = parse_contract(table.get("feature_contract"), diagnostics);
    if mapping.is_some() && contract.is_none() && !table.contains_key("feature_contract") {
        diagnostics.push(Diagnostic::error(
            "feature_contract",
            "is required when [validation] maps a feature",
        ));
    }
    if mapping.is_none() && contract.is_some() && !table.contains_key("validation") {
        diagnostics.push(Diagnostic::error(
            "validation",
            "is required when [feature_contract] is declared",
        ));
    }
    (mapping, contract)
}

fn parse_mapping(
    value: Option<&Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<FeatureScenarioMapping> {
    let value = value?;
    let Some(table) = value.as_table() else {
        diagnostics.push(Diagnostic::error("validation", "must be a table"));
        return None;
    };
    let feature = issue_key(table, "feature", true, diagnostics);
    let plan = issue_key(table, "plan", false, diagnostics);
    let source_branch = required_string(table, "source_branch", "validation", diagnostics);
    if let Some(branch) = source_branch.as_deref() {
        if let Err(message) = validate_source_branch(branch) {
            diagnostics.push(Diagnostic::error("validation.source_branch", message));
        }
    }
    let change = required_string(table, "change", "validation", diagnostics).and_then(|raw| {
        FeatureMappingChange::parse(&raw).or_else(|| {
            diagnostics.push(Diagnostic::error(
                "validation.change",
                format!("unknown change intent `{raw}` (expected `new` or `updated`)"),
            ));
            None
        })
    });

    match (feature, source_branch, change) {
        (Some(feature), Some(source_branch), Some(change))
            if validate_source_branch(&source_branch).is_ok() =>
        {
            Some(FeatureScenarioMapping {
                feature,
                plan,
                source_branch,
                change,
            })
        }
        _ => None,
    }
}

fn parse_contract(
    value: Option<&Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ScenarioFeatureContract> {
    let value = value?;
    let Some(table) = value.as_table() else {
        diagnostics.push(Diagnostic::error("feature_contract", "must be a table"));
        return None;
    };
    let claim = required_string(table, "claim", "feature_contract", diagnostics);
    let stimulus = required_string(table, "stimulus", "feature_contract", diagnostics);
    let observable = required_string(table, "observable", "feature_contract", diagnostics);
    let assertion = required_string(table, "assertion", "feature_contract", diagnostics);
    let jig_script_path =
        required_string(table, "jig_script_path", "feature_contract", diagnostics);
    let runtime_budget_seconds = match table.get("runtime_budget_seconds") {
        Some(value) => match value
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
        {
            Some(value @ 1..=3600) => Some(value),
            _ => {
                diagnostics.push(Diagnostic::error(
                    "feature_contract.runtime_budget_seconds",
                    "must be an integer from 1 through 3600",
                ));
                None
            }
        },
        None => {
            diagnostics.push(Diagnostic::error(
                "feature_contract.runtime_budget_seconds",
                "required field is missing",
            ));
            None
        }
    };

    match (
        claim,
        stimulus,
        observable,
        assertion,
        runtime_budget_seconds,
        jig_script_path,
    ) {
        (
            Some(claim),
            Some(stimulus),
            Some(observable),
            Some(assertion),
            Some(runtime_budget_seconds),
            Some(jig_script_path),
        ) => Some(ScenarioFeatureContract {
            claim,
            stimulus,
            observable,
            assertion,
            runtime_budget_seconds,
            jig_script_path,
        }),
        _ => None,
    }
}

fn issue_key(
    table: &toml::Table,
    key: &str,
    required: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ForgeIssueKey> {
    let field = format!("validation.{key}");
    let Some(value) = table.get(key) else {
        if required {
            diagnostics.push(Diagnostic::error(field, "required field is missing"));
        }
        return None;
    };
    let raw = string_value(&field, value, diagnostics)?;
    raw.parse()
        .map_err(|message| {
            diagnostics.push(Diagnostic::error(&field, message));
        })
        .ok()
}

fn required_string(
    table: &toml::Table,
    key: &str,
    section: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let field = format!("{section}.{key}");
    let Some(value) = table.get(key) else {
        diagnostics.push(Diagnostic::error(field, "required field is missing"));
        return None;
    };
    string_value(field, value, diagnostics)
}
