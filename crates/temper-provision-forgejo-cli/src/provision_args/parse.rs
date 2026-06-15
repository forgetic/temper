//! Argument parsing for `temper provision-forgejo`.

use std::path::PathBuf;

use crate::provision::AccessScope;

use super::model::{
    ADMIN_TOKEN_ENV, ArgsError, ParseOutcome, ProvisionArgs, USAGE, WORKFLOW_FILE_ENV,
};

pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    parse_with_env(args, |key| std::env::var(key).ok())
}

pub fn parse_with_env<I, E>(args: I, env: E) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
    E: Fn(&str) -> Option<String>,
{
    let mut base_url = None;
    let mut owner = None;
    let mut name = None;
    let mut out = None;
    let mut webhook_url = None;
    let mut webhook_secret_file = None;
    let mut seed_intake = true;
    let mut seed_only = false;
    let mut intake_title = None;
    let mut intake_body_file = None;
    let mut workflow_file = None;
    let mut existing_repo = false;
    let mut access = AccessScope::default();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--base-url" => base_url = Some(value_for(&flag, &mut iter)?),
            "--owner" => owner = Some(value_for(&flag, &mut iter)?),
            "--name" => name = Some(value_for(&flag, &mut iter)?),
            "--out" => out = Some(value_for(&flag, &mut iter)?),
            "--workflow" => workflow_file = Some(value_for(&flag, &mut iter)?),
            "--webhook-url" => webhook_url = Some(value_for(&flag, &mut iter)?),
            "--webhook-secret-file" => webhook_secret_file = Some(value_for(&flag, &mut iter)?),
            "--seed-intake" => seed_intake = parse_bool(&value_for(&flag, &mut iter)?)?,
            "--seed-only" => seed_only = true,
            "--existing-repo" => existing_repo = true,
            "--access" => access = parse_access(&value_for(&flag, &mut iter)?)?,
            "--intake-title" => intake_title = Some(value_for(&flag, &mut iter)?),
            "--intake-body-file" => intake_body_file = Some(value_for(&flag, &mut iter)?),
            other => {
                return Err(ArgsError::new(format!(
                    "unrecognized argument '{other}'\nusage: {USAGE}"
                )));
            }
        }
    }
    let admin_token = env(ADMIN_TOKEN_ENV)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ArgsError::new(format!(
                "missing required environment variable {ADMIN_TOKEN_ENV}"
            ))
        })?;
    if seed_only && !seed_intake {
        return Err(ArgsError::new(
            "--seed-only files the intake issue and so cannot be combined with --seed-intake no",
        ));
    }
    Ok(ParseOutcome::Run(ProvisionArgs {
        base_url: require(base_url, "--base-url")?,
        owner: require(owner, "--owner")?,
        name: require(name, "--name")?,
        out: PathBuf::from(require(out, "--out")?),
        admin_token,
        webhook_url,
        webhook_secret_file: webhook_secret_file.map(PathBuf::from),
        seed_intake,
        seed_only,
        intake_title,
        intake_body_file: intake_body_file.map(PathBuf::from),
        workflow_file: non_empty(workflow_file)
            .or_else(|| non_empty(env(WORKFLOW_FILE_ENV)))
            .map(PathBuf::from),
        existing_repo,
        access,
    }))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then_some(value))
}

fn value_for<I>(flag: &str, iter: &mut I) -> Result<String, ArgsError>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| ArgsError::new(format!("flag '{flag}' expects a value")))
}

fn require(value: Option<String>, flag: &str) -> Result<String, ArgsError> {
    value.ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

fn parse_bool(value: &str) -> Result<bool, ArgsError> {
    match value {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        other => Err(ArgsError::new(format!(
            "--seed-intake expects yes|no, got '{other}'"
        ))),
    }
}

fn parse_access(value: &str) -> Result<AccessScope, ArgsError> {
    match value {
        "org-owners" => Ok(AccessScope::OrgOwners),
        "repo-collaborator" => Ok(AccessScope::RepoCollaborator),
        other => Err(ArgsError::new(format!(
            "--access expects org-owners|repo-collaborator, got '{other}'"
        ))),
    }
}
