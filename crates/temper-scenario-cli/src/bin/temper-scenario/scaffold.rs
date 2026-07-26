// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use temper_scenario_core::{
    FeatureMappingChange, ForgeIssueKey, check_scenario, validate_source_branch,
};
use temper_testing::live_manifest::ScenarioBundle;

const EX_USAGE: u8 = 64;
const DEFAULT_BUDGET_SECONDS: u64 = 600;

const USAGE: &str = "\
Create a minimal inherited, feature-mapped live scenario bundle.\n\
\n\
Usage: temper-scenario scaffold --feature <OWNER/REPO#N> --plan <OWNER/REPO#N> --source-branch <BRANCH> --name <SLUG> [OPTIONS]\n\
\n\
Required options:\n\
  --feature <OWNER/REPO#N>  Feature issue mapped to this scenario\n\
  --plan <OWNER/REPO#N>     Plan issue that supplied authoring context\n\
  --source-branch <BRANCH>  Feature branch whose exact head will be validated\n\
  --name <SLUG>             Scenario directory/name (lowercase letters, digits, hyphens)\n\
\n\
Options:\n\
  --scenarios-dir <DIR>           Scenario corpus root (default: scenarios)\n\
  --extends <PATH>                Inherited fixture bundle (default: scenarios/basic-delivery)\n\
  --change <new|updated>          Landing-base intent (default: new)\n\
  --runtime-budget-seconds <N>    Bounded runtime budget, 1..3600 (default: 600)\n\
  --claim <TEXT>                  Feature claim\n\
  --stimulus <TEXT>               Bounded stimulus description\n\
  --observable <TEXT>             Structured observable description\n\
  --assertion <TEXT>              Required assertion description\n\
  -h, --help                      Print help\n\
\n\
The command creates scenario.toml, README.md, and a local Jig script. It never\n\
creates credentials or runtime artifacts and refuses to overwrite an existing path.";

pub(super) fn command(args: &[String]) -> ExitCode {
    let args = match parse_args(args) {
        Ok(ParseResult::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };
    let target = args.scenarios_dir.join(&args.name);
    match create_bundle(&target, &args) {
        Ok(()) => {
            println!("{}", target.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("temper-scenario scaffold: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Args {
    feature: ForgeIssueKey,
    plan: ForgeIssueKey,
    source_branch: String,
    name: String,
    scenarios_dir: PathBuf,
    extends: String,
    change: FeatureMappingChange,
    runtime_budget_seconds: u64,
    claim: String,
    stimulus: String,
    observable: String,
    assertion: String,
}

enum ParseResult {
    Help,
    Args(Box<Args>),
}

fn parse_args(args: &[String]) -> Result<ParseResult, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseResult::Help);
    }
    let mut values = std::collections::BTreeMap::<String, String>::new();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        if !flag.starts_with("--") {
            return usage_error(format!("unexpected argument `{flag}`"));
        }
        let Some(value) = args.get(index + 1) else {
            return usage_error(format!("{flag} requires a value"));
        };
        if value.starts_with("--") {
            return usage_error(format!("{flag} requires a value"));
        }
        if !matches!(
            flag.as_str(),
            "--feature"
                | "--plan"
                | "--source-branch"
                | "--name"
                | "--scenarios-dir"
                | "--extends"
                | "--change"
                | "--runtime-budget-seconds"
                | "--claim"
                | "--stimulus"
                | "--observable"
                | "--assertion"
        ) {
            return usage_error(format!("unexpected option `{flag}`"));
        }
        if values.insert(flag.clone(), value.clone()).is_some() {
            return usage_error(format!("duplicate {flag} option"));
        }
        index += 2;
    }

    let feature = issue_value(&values, "--feature")?;
    let plan = issue_value(&values, "--plan")?;
    let source_branch = required_value(&values, "--source-branch")?.to_string();
    if let Err(message) = validate_source_branch(&source_branch) {
        return usage_error(format!("--source-branch {message}"));
    }
    let name = required_value(&values, "--name")?.to_string();
    if !valid_slug(&name) {
        return usage_error(
            "--name must contain only lowercase ASCII letters, digits, and single hyphens"
                .to_string(),
        );
    }
    let extends = values
        .get("--extends")
        .cloned()
        .unwrap_or_else(|| "scenarios/basic-delivery".to_string());
    if !safe_relative_path(&extends) {
        return usage_error(
            "--extends must be a local relative path without `..` components".to_string(),
        );
    }
    let change = values
        .get("--change")
        .map(String::as_str)
        .map(FeatureMappingChange::parse)
        .unwrap_or(Some(FeatureMappingChange::New))
        .ok_or_else(|| {
            eprintln!("temper-scenario scaffold: --change must be `new` or `updated`\n\n{USAGE}");
        })?;
    let runtime_budget_seconds = match values.get("--runtime-budget-seconds") {
        Some(value) => value.parse::<u64>().map_err(|_| {
            eprintln!(
                "temper-scenario scaffold: --runtime-budget-seconds must be an integer from 1 through 3600\n\n{USAGE}"
            );
        })?,
        None => DEFAULT_BUDGET_SECONDS,
    };
    if !(1..=3600).contains(&runtime_budget_seconds) {
        return usage_error(
            "--runtime-budget-seconds must be an integer from 1 through 3600".to_string(),
        );
    }

    let claim = optional_text(&values, "--claim")
        .unwrap_or_else(|| format!("Feature {feature} satisfies the contract planned by {plan}."));
    let stimulus = optional_text(&values, "--stimulus").unwrap_or_else(|| {
        "Deliver the focused feature workflow through the inherited live stack.".to_string()
    });
    let observable = optional_text(&values, "--observable").unwrap_or_else(|| {
        "Structured Forge, CI, Temper event, and Jig request facts.".to_string()
    });
    let assertion = optional_text(&values, "--assertion").unwrap_or_else(|| {
        "Every required assertion passes at the exact feature head.".to_string()
    });

    Ok(ParseResult::Args(Box::new(Args {
        feature,
        plan,
        source_branch,
        name,
        scenarios_dir: values
            .get("--scenarios-dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("scenarios")),
        extends,
        change,
        runtime_budget_seconds,
        claim,
        stimulus,
        observable,
        assertion,
    })))
}

fn create_bundle(target: &Path, args: &Args) -> Result<(), String> {
    fs::create_dir_all(&args.scenarios_dir).map_err(|error| {
        format!(
            "create scenario root {}: {error}",
            args.scenarios_dir.display()
        )
    })?;
    fs::create_dir(target).map_err(|error| {
        format!(
            "refusing to overwrite scenario path {}: {error}",
            target.display()
        )
    })?;
    let result = (|| {
        let jig_dir = target.join("jig");
        fs::create_dir(&jig_dir)
            .map_err(|error| format!("create {}: {error}", jig_dir.display()))?;
        write_file(&target.join("scenario.toml"), &render_manifest(args))?;
        write_file(&target.join("README.md"), &render_readme(args))?;
        let jig = serde_json::to_string_pretty(&jig_document())
            .map_err(|error| format!("render Jig script: {error}"))?;
        write_file(&jig_dir.join(format!("{}.json", args.name)), &(jig + "\n"))?;

        let report = check_scenario(target);
        if !report.is_valid() {
            return Err(format!(
                "generated bundle failed manifest validation: {}",
                report
                    .diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let bundle = ScenarioBundle::load(target)
            .map_err(|error| format!("generated bundle is not scenario-ready: {error}"))?;
        bundle
            .validate_workflow()
            .map_err(|error| format!("generated bundle workflow is invalid: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(target);
    }
    result
}

fn render_manifest(args: &Args) -> String {
    let q = |value: &str| toml::Value::String(value.to_string()).to_string();
    format!(
        "schema = \"temper.scenario.v1\"\nname = {name}\nstatus = \"active\"\nstability = \"provisional\"\nintent = {claim}\ntimeout = {budget}\n\n[fixtures]\nextends = {extends}\n\n[runner]\nuses = \"manifest\"\n\n[jig]\nscript_path = {jig}\n\n[validation]\nfeature = {feature}\nplan = {plan}\nsource_branch = {branch}\nchange = \"{change}\"\n\n[feature_contract]\nclaim = {claim}\nstimulus = {stimulus}\nobservable = {observable}\nassertion = {assertion}\nruntime_budget_seconds = {budget}\njig_script_path = {jig}\n",
        name = q(&args.name),
        claim = q(&args.claim),
        budget = args.runtime_budget_seconds,
        extends = q(&args.extends),
        jig = q(&format!("jig/{}.json", args.name)),
        feature = q(&args.feature.to_string()),
        plan = q(&args.plan.to_string()),
        branch = q(&args.source_branch),
        change = args.change,
        stimulus = q(&args.stimulus),
        observable = q(&args.observable),
        assertion = q(&args.assertion),
    )
}

fn render_readme(args: &Args) -> String {
    format!(
        "# {}\n\nMapped feature: `{}`  \nPlan context: `{}`  \nSource branch: `{}`\n\n## Claim → stimulus → observable → assertion\n\n- **Claim:** {}\n- **Stimulus:** {}\n- **Observable:** {}\n- **Assertion:** {}\n- **Runtime budget:** {} seconds\n\nThis bundle inherits stable live-stack fixtures from `{}`. Edit the local Jig script and focused assertions for the feature; do not add credentials, generated logs, or runtime state.\n",
        args.name,
        args.feature,
        args.plan,
        args.source_branch,
        args.claim,
        args.stimulus,
        args.observable,
        args.assertion,
        args.runtime_budget_seconds,
        args.extends,
    )
}

fn jig_document() -> serde_json::Value {
    json!({
        "phases": [
            {
                "name": "architect-triage",
                "when": { "messages_contain": ["ROLE: architect"] },
                "sequence": [{
                    "text": "{\"verdict\":\"ready_code\",\"body\":\"Implement the focused feature behavior and preserve structured evidence.\"}"
                }]
            },
            {
                "name": "engineer-implementation",
                "when": { "messages_contain": ["ROLE: engineer"] },
                "sequence": [{
                    "text": "{\"title\":\"Implement focused feature proof\",\"body\":\"# Implementation report\\n\\nReplace this scaffold response with deterministic tool calls for the feature scenario.\",\"summary\":\"Prepared the focused feature proof.\"}"
                }]
            }
        ]
    })
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|error| format!("write {}: {error}", path.display()))
}

fn required_value<'a>(
    values: &'a std::collections::BTreeMap<String, String>,
    flag: &str,
) -> Result<&'a str, ()> {
    values
        .get(flag)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            eprintln!("temper-scenario scaffold: missing {flag}\n\n{USAGE}");
        })
}

fn issue_value(
    values: &std::collections::BTreeMap<String, String>,
    flag: &str,
) -> Result<ForgeIssueKey, ()> {
    required_value(values, flag)?.parse().map_err(|message| {
        eprintln!("temper-scenario scaffold: {flag} {message}\n\n{USAGE}");
    })
}

fn optional_text(
    values: &std::collections::BTreeMap<String, String>,
    flag: &str,
) -> Option<String> {
    values
        .get(flag)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains("://")
        && !Path::new(value).is_absolute()
        && !Path::new(value).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn usage_error<T>(message: String) -> Result<T, ()> {
    eprintln!("temper-scenario scaffold: {message}\n\n{USAGE}");
    Err(())
}
