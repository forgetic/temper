// SPDX-License-Identifier: MPL-2.0

//! Worker-owned execution of the mapped validation scenario.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::Command;

use temper_protocol_agent::WorkspaceResult;
use temper_protocol_worker::FailureClass;
use temper_scenario_core::{FollowUpIssueIntent, ValidationVerdict, ValidatorResult};

use crate::executor::{JobCancellation, JobOutcome};
use crate::managed_effect::{
    JoinedBlocking, ManagedCommand, ManagedCommandCapture, WORKER_COMMAND_COMPLETE_BYTES,
    WORKER_COMMAND_TAIL_BYTES,
};

use super::{
    CONTENT_DIGEST_FIELD, EXACT_HEAD_FIELD, FEATURE_FIELD, MAPPING_ID_FIELD, PLAN_FIELD,
    SCENARIO_NAME_FIELD, SCENARIO_PATH_FIELD, SOURCE_BRANCH_FIELD, required_metadata,
};
use crate::coding_executor::{PreparedRepo, failure};

const STDOUT_FILE: &str = "validator.stdout.log";
const STDERR_FILE: &str = "validator.stderr.log";

/// Program plus fixed arguments used to invoke the checked-out validator CLI.
#[derive(Clone, Debug)]
pub struct NativeValidatorCommand {
    program: OsString,
    prefix_args: Vec<OsString>,
}

impl NativeValidatorCommand {
    /// Builds an override used by hermetic worker tests or custom packaging.
    #[doc(hidden)]
    pub fn new(
        program: impl Into<OsString>,
        prefix_args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            prefix_args: prefix_args.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn cargo() -> Self {
        Self::new(
            "cargo",
            [
                "run",
                "--quiet",
                "--bin",
                "temper-scenario",
                "--",
                "validate",
            ],
        )
    }
}

pub(crate) async fn run(
    command_spec: &NativeValidatorCommand,
    metadata: &temper_verdict::SourceMetadata,
    prepared: &PreparedRepo,
    repo: &str,
    plan_number: u64,
    credential_roles: &[String],
    cancellation: &JobCancellation,
) -> Result<WorkspaceResult, JobOutcome> {
    let checkout = prepared.workspace.path();
    let exact_head = required_metadata(metadata, EXACT_HEAD_FIELD)?;
    let scenario_path = required_metadata(metadata, SCENARIO_PATH_FIELD)?;
    let output_dir = checkout
        .parent()
        .unwrap_or(checkout)
        .join(".temper-validation")
        .join(exact_head);
    prepare_native_workspace(&output_dir, checkout).await?;

    let plan_number = plan_number.to_string();
    let mut command = Command::new(&command_spec.program);
    command.args(&command_spec.prefix_args).args([
        OsStr::new("--pr"),
        OsStr::new(&plan_number),
        OsStr::new("--sha"),
        OsStr::new(exact_head),
        OsStr::new("--scenario"),
        OsStr::new(scenario_path),
        OsStr::new("--tier"),
        OsStr::new("live"),
        OsStr::new("--output-dir"),
        output_dir.as_os_str(),
        OsStr::new("--repo"),
        OsStr::new(repo),
        OsStr::new("--target-kind"),
        OsStr::new("plan"),
        OsStr::new("--target-issue"),
        OsStr::new(&plan_number),
    ]);
    command.current_dir(checkout);
    remove_forge_credentials(&mut command, credential_roles);
    for key in [
        "TEMPER_SCENARIO_TEMPER_BIN",
        "TEMPER_BIN",
        "CARGO_TARGET_DIR",
    ] {
        command.env_remove(key);
    }
    for (key, field) in [
        ("TEMPER_VALIDATION_FEATURE", FEATURE_FIELD),
        ("TEMPER_VALIDATION_PLAN", PLAN_FIELD),
        ("TEMPER_VALIDATION_MAPPING", MAPPING_ID_FIELD),
        ("TEMPER_VALIDATION_SCENARIO_NAME", SCENARIO_NAME_FIELD),
        ("TEMPER_VALIDATION_SCENARIO_PATH", SCENARIO_PATH_FIELD),
        ("TEMPER_VALIDATION_SOURCE_BRANCH", SOURCE_BRANCH_FIELD),
        ("TEMPER_VALIDATION_HEAD", EXACT_HEAD_FIELD),
        ("TEMPER_VALIDATION_CONTENT_DIGEST", CONTENT_DIGEST_FIELD),
    ] {
        command.env(key, required_metadata(metadata, field)?);
    }
    command.env("TEMPER_VALIDATION_REPO", repo);
    command.env("TEMPER_VALIDATION_OUTPUT", &output_dir);

    let output = ManagedCommand::spawn(
        command,
        cancellation.clone(),
        ManagedCommandCapture::new(
            temper_process_containment::CaptureMode::Complete,
            WORKER_COMMAND_COMPLETE_BYTES,
            temper_process_containment::CaptureMode::Tail,
            WORKER_COMMAND_TAIL_BYTES,
        ),
    )
    .await
    .map_err(|error| {
        failure(
            if error.kind() == std::io::ErrorKind::Interrupted {
                FailureClass::Canceled
            } else {
                FailureClass::Transient
            },
            format!("run workflow-native exact-head validator: {error}"),
        )
    })?;

    let stdout_path = output_dir.join(STDOUT_FILE);
    let stderr_path = output_dir.join(STDERR_FILE);
    write_command_logs(
        &stdout_path,
        &output.stdout,
        &stderr_path,
        &output.stderr,
        output.stderr_dropped_bytes,
    )
    .await?;

    let mut evidence = load_validator_result(&output_dir).await?;
    if evidence.verdict == ValidationVerdict::Passed && !output.status.success() {
        return Err(failure(
            FailureClass::Protocol,
            format!(
                "validator process exited with {} but claimed a passing result",
                output.status
            ),
        ));
    }
    if evidence.verdict != ValidationVerdict::Passed && evidence.follow_up_issue.is_none() {
        evidence.follow_up_issue = Some(
            FollowUpIssueIntent::new(
                format!("Repair exact-head validation for {scenario_path}"),
                format!(
                    "The workflow-native validator returned `{}` for `{scenario_path}` at `{exact_head}`. Inspect the retained validation artifacts and repair the product, observability, or scenario harness before validation is retried.",
                    evidence.verdict
                ),
            )
            .with_label("scenario-harness"),
        );
    }
    push_retained(&mut evidence.retained_paths, &stdout_path);
    push_retained(&mut evidence.retained_paths, &stderr_path);

    let body = serde_json::to_string(&evidence).map_err(|error| {
        failure(
            FailureClass::Protocol,
            format!("serialize native validator result: {error}"),
        )
    })?;
    Ok(WorkspaceResult {
        body: Some(body),
        ..WorkspaceResult::default()
    })
}

async fn prepare_native_workspace(
    output_dir: &Path,
    checkout_root: &Path,
) -> Result<(), JobOutcome> {
    let output_dir = output_dir.to_path_buf();
    let standalone = checkout_root
        .join("target/debug")
        .join(format!("temper{}", std::env::consts::EXE_SUFFIX));
    JoinedBlocking::spawn("temper-validator-output", move || {
        if output_dir.exists() {
            fs::remove_dir_all(&output_dir)?;
        }
        fs::create_dir_all(&output_dir)?;
        match fs::remove_file(&standalone) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    })
    .await
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("join validator output reset: {error}"),
        )
    })?
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("reset validator output: {error}"),
        )
    })
}

async fn write_command_logs(
    stdout_path: &Path,
    stdout: &[u8],
    stderr_path: &Path,
    stderr: &[u8],
    stderr_dropped: u64,
) -> Result<(), JobOutcome> {
    let stdout_path = stdout_path.to_path_buf();
    let stdout = stdout.to_vec();
    let stderr_path = stderr_path.to_path_buf();
    let mut stderr = stderr.to_vec();
    if stderr_dropped > 0 {
        stderr.extend_from_slice(
            format!("\n[Temper omitted {stderr_dropped} earlier stderr byte(s).]\n").as_bytes(),
        );
    }
    JoinedBlocking::spawn("temper-validator-logs", move || {
        fs::write(stdout_path, stdout)?;
        fs::write(stderr_path, stderr)
    })
    .await
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("join validator log write: {error}"),
        )
    })?
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("write validator logs: {error}"),
        )
    })
}

async fn load_validator_result(output_dir: &Path) -> Result<ValidatorResult, JobOutcome> {
    let output_dir = output_dir.to_path_buf();
    JoinedBlocking::spawn("temper-validator-result", move || {
        let mut results = Vec::new();
        for entry in fs::read_dir(&output_dir)? {
            let path = entry?.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            if let Ok(result) = serde_json::from_slice::<ValidatorResult>(&bytes) {
                results.push((path, result));
            }
        }
        match results.as_mut_slice() {
            [(_, result)] => Ok(result.clone()),
            [] => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "validator produced no typed ValidatorResult JSON",
            )),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "validator produced multiple typed ValidatorResult JSON files",
            )),
        }
    })
    .await
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("join validator result read: {error}"),
        )
    })?
    .map_err(|error| failure(FailureClass::Protocol, error.to_string()))
}

fn remove_forge_credentials(command: &mut Command, roles: &[String]) {
    for key in [
        "TEMPER_FORGE_TOKEN",
        "TEMPER_FORGEJO_TOKEN",
        "TEMPER_FORGEJO_ADMIN_TOKEN",
        "FORGEJO_ACCESS_TOKEN",
        "GITHUB_TOKEN",
    ] {
        command.env_remove(key);
    }
    for role in roles {
        let role_key = role
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>();
        command.env_remove(format!("TEMPER_FORGEJO_TOKEN_{role_key}"));
    }
}

fn push_retained(paths: &mut Vec<String>, path: &Path) {
    let value = path.display().to_string();
    if !paths.contains(&value) {
        paths.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_validator_command_explicitly_removes_mutation_credentials() {
        let mut command = Command::new("validator");
        command.env("TEMPER_FORGE_TOKEN", "secret");
        command.env("FORGEJO_ACCESS_TOKEN", "secret");
        command.env("GITHUB_TOKEN", "secret");
        remove_forge_credentials(&mut command, &["tester".to_string()]);
        let changes = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
            .collect::<std::collections::BTreeMap<_, _>>();
        for key in ["TEMPER_FORGE_TOKEN", "FORGEJO_ACCESS_TOKEN", "GITHUB_TOKEN"] {
            assert_eq!(changes.get(OsStr::new(key)), Some(&None));
        }
    }
}
