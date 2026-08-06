// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use temper_protocol_agent::SubmitForPrResponse;
use temper_worker::{
    AcceptedSubmitProof, WorkspaceFingerprint, fingerprint_writable_repos_blocking,
};

use super::BenchmarkRunError;
use crate::{PreparedBenchmarkWorkspace, ResolvedBenchmarkManifest, ValidationEvidenceV1};

const OUTPUT_TAIL_BYTES: usize = 64 * 1024;
type TailReader = Option<thread::JoinHandle<io::Result<(Vec<u8>, u64)>>>;

/// Version for the detailed host-validation artifact.
pub const VALIDATION_ARTIFACT_VERSION: u32 = 1;

/// Host-side evidence gathered after the measured agent wall time ends.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationArtifactV1 {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_submit: Option<AcceptedSubmitEvidenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_patch: Option<ExactPatchEvidenceV1>,
    pub post_run_commands: Vec<ValidationCommandEvidenceV1>,
}

/// Host-owned comparison of the final writable-repository diff with the
/// checked-in expected patch. Patch content is not duplicated into reports.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactPatchEvidenceV1 {
    pub expected_patch: String,
    pub status: String,
    pub untracked_files: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

/// The worker-owned accepted proof plus a fresh post-session comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedSubmitEvidenceV1 {
    pub response: SubmitForPrResponse,
    pub fingerprint: WorkspaceFingerprint,
    pub fingerprint_current_after_session: bool,
}

/// One manifest post-run command and its bounded diagnostic tails.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationCommandEvidenceV1 {
    pub argv: Vec<String>,
    pub cwd: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub stdout_dropped_bytes: u64,
    pub stderr_dropped_bytes: u64,
}

pub(super) fn accepted_submit_evidence(
    proof: Option<&AcceptedSubmitProof>,
    workspace: &PreparedBenchmarkWorkspace,
    repetition: u32,
) -> Result<Option<AcceptedSubmitEvidenceV1>, BenchmarkRunError> {
    let Some(proof) = proof else {
        return Ok(None);
    };
    let current = fingerprint_writable_repos_blocking(workspace.context(), workspace.root())
        .map_err(|error| BenchmarkRunError::Fingerprint {
            repetition,
            message: error.to_string(),
        })?;
    Ok(Some(AcceptedSubmitEvidenceV1 {
        response: proof.response.clone(),
        fingerprint: proof.fingerprint.clone(),
        fingerprint_current_after_session: proof.fingerprint == current,
    }))
}

pub(super) fn validation_summary(artifact: &ValidationArtifactV1) -> ValidationEvidenceV1 {
    let gates = artifact
        .accepted_submit
        .as_ref()
        .map_or(&[][..], |proof| proof.response.gates.as_slice());
    let gate_succeeded = gates
        .iter()
        .filter(|gate| {
            !gate.timed_out
                && gate.exit_code == Some(0)
                && gate.exit_status.eq_ignore_ascii_case("passed")
        })
        .count() as u64;
    let command_succeeded = artifact
        .post_run_commands
        .iter()
        .filter(|command| command.status == "passed")
        .count() as u64;
    let exact_count = u64::from(artifact.exact_patch.is_some());
    let exact_succeeded = u64::from(
        artifact
            .exact_patch
            .as_ref()
            .is_some_and(|evidence| evidence.status == "passed"),
    );
    let command_count = gates.len() as u64 + artifact.post_run_commands.len() as u64 + exact_count;
    let succeeded = gate_succeeded + command_succeeded + exact_succeeded;
    ValidationEvidenceV1 {
        command_count,
        succeeded,
        failed: command_count.saturating_sub(succeeded),
    }
}

pub(super) fn validate_expected_patch(
    manifest: &ResolvedBenchmarkManifest,
    workspace: &PreparedBenchmarkWorkspace,
    untracked_files: u64,
) -> Result<Option<ExactPatchEvidenceV1>, BenchmarkRunError> {
    let Some(expected_path) = manifest.expected_patch_path() else {
        return Ok(None);
    };
    let declared = manifest
        .manifest()
        .expected_patch
        .as_ref()
        .expect("resolved expected patch has declaration")
        .display()
        .to_string();
    let writable = workspace
        .context()
        .repos
        .iter()
        .filter(|repository| repository.access == "writable")
        .collect::<Vec<_>>();
    if writable.len() != 1 {
        return Ok(Some(failed_exact_patch(
            declared,
            untracked_files,
            "exact patch validation requires exactly one writable repository",
        )));
    }
    let repository = writable[0];
    let Some(baseline) = workspace
        .baselines()
        .iter()
        .find(|baseline| baseline.id == repository.id)
    else {
        return Ok(Some(failed_exact_patch(
            declared,
            untracked_files,
            "writable repository baseline is unavailable",
        )));
    };
    let cwd = workspace.root().join(&repository.dir);
    let output = Command::new("git")
        .args([
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-renames",
            &baseline.sha,
            "--",
            ".",
        ])
        .current_dir(&cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|source| BenchmarkRunError::Io {
            operation: "run exact patch validation",
            path: cwd.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(BenchmarkRunError::Git {
            command: "git diff --binary --no-ext-diff --no-renames <baseline> -- .".to_string(),
            cwd,
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let expected = fs::read(expected_path).map_err(|source| BenchmarkRunError::Io {
        operation: "read expected patch",
        path: expected_path.to_path_buf(),
        source,
    })?;
    let matches = untracked_files == 0
        && canonical_text_patch(&expected).is_some_and(|expected| {
            canonical_text_patch(&output.stdout).is_some_and(|actual| actual == expected)
        });
    Ok(Some(if matches {
        ExactPatchEvidenceV1 {
            expected_patch: declared,
            status: "passed".to_string(),
            untracked_files,
            diagnostic: None,
        }
    } else {
        failed_exact_patch(
            declared,
            untracked_files,
            "final diff does not exactly match the checked-in expected patch",
        )
    }))
}

fn canonical_text_patch(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    Some(
        text.split_inclusive('\n')
            .filter(|line| !line.starts_with("index "))
            .collect::<String>()
            .into_bytes(),
    )
}

fn failed_exact_patch(
    expected_patch: String,
    untracked_files: u64,
    diagnostic: &str,
) -> ExactPatchEvidenceV1 {
    ExactPatchEvidenceV1 {
        expected_patch,
        status: "failed".to_string(),
        untracked_files,
        diagnostic: Some(diagnostic.to_string()),
    }
}

pub(super) fn run_post_run_commands(
    manifest: &ResolvedBenchmarkManifest,
    workspace: &PreparedBenchmarkWorkspace,
) -> Vec<ValidationCommandEvidenceV1> {
    manifest
        .manifest()
        .post_run_commands
        .iter()
        .map(|argv| run_validation_command(argv, workspace.root()))
        .collect()
}

fn run_validation_command(argv: &[String], cwd: &Path) -> ValidationCommandEvidenceV1 {
    let started = Instant::now();
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let cwd_display = cwd.display().to_string();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ValidationCommandEvidenceV1 {
                argv: argv.to_vec(),
                cwd: cwd_display,
                status: "spawn_failed".to_string(),
                exit_code: None,
                timed_out: false,
                duration_ms: elapsed_ms(started),
                stdout_tail: String::new(),
                stderr_tail: error.to_string(),
                stdout_dropped_bytes: 0,
                stderr_dropped_bytes: 0,
            };
        }
    };

    let stdout = child
        .stdout
        .take()
        .map(|stream| thread::spawn(move || capture_tail(stream, OUTPUT_TAIL_BYTES)));
    let stderr = child
        .stderr
        .take()
        .map(|stream| thread::spawn(move || capture_tail(stream, OUTPUT_TAIL_BYTES)));
    let waited = child.wait();
    let stdout = join_tail(stdout, "stdout");
    let stderr = join_tail(stderr, "stderr");
    let (status, exit_code, wait_error) = match waited {
        Ok(status) => (
            if status.success() { "passed" } else { "failed" }.to_string(),
            status.code(),
            None,
        ),
        Err(error) => ("wait_failed".to_string(), None, Some(error.to_string())),
    };
    let mut stderr_tail = stderr.text;
    for diagnostic in [stdout.error, stderr.error, wait_error]
        .into_iter()
        .flatten()
    {
        if !stderr_tail.is_empty() {
            stderr_tail.push('\n');
        }
        stderr_tail.push_str(&diagnostic);
    }

    ValidationCommandEvidenceV1 {
        argv: argv.to_vec(),
        cwd: cwd_display,
        status,
        exit_code,
        timed_out: false,
        duration_ms: elapsed_ms(started),
        stdout_tail: stdout.text,
        stderr_tail,
        stdout_dropped_bytes: stdout.dropped,
        stderr_dropped_bytes: stderr.dropped,
    }
}

struct CapturedTail {
    text: String,
    dropped: u64,
    error: Option<String>,
}

fn capture_tail(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, u64)> {
    let mut retained = Vec::new();
    let mut dropped = 0_u64;
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        retained.extend_from_slice(&chunk[..read]);
        if retained.len() > limit {
            let excess = retained.len() - limit;
            retained.drain(..excess);
            dropped = dropped.saturating_add(excess as u64);
        }
    }
    Ok((retained, dropped))
}

fn join_tail(thread: TailReader, stream: &str) -> CapturedTail {
    match thread {
        Some(thread) => match thread.join() {
            Ok(Ok((bytes, dropped))) => CapturedTail {
                text: String::from_utf8_lossy(&bytes).into_owned(),
                dropped,
                error: None,
            },
            Ok(Err(error)) => CapturedTail {
                text: String::new(),
                dropped: 0,
                error: Some(format!("read {stream}: {error}")),
            },
            Err(_) => CapturedTail {
                text: String::new(),
                dropped: 0,
                error: Some(format!("{stream} reader thread panicked")),
            },
        },
        None => CapturedTail {
            text: String::new(),
            dropped: 0,
            error: Some(format!("{stream} pipe was unavailable")),
        },
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_benchmark_manifest, prepare_benchmark_workspace};

    #[test]
    fn exact_patch_gate_rejects_extra_or_different_changes() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("fixture/repo");
        fs::create_dir_all(&fixture).unwrap();
        fs::write(fixture.join("value.txt"), "before\n").unwrap();
        fs::write(root.path().join("expected.patch"), "placeholder\n").unwrap();
        fs::write(root.path().join("jig.json"), "{}\n").unwrap();
        fs::write(
            root.path().join("context.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "repos": [{
                    "id": "repo-1",
                    "owner": "acme",
                    "name": "fixture",
                    "default_branch": "main",
                    "dir": "repo",
                    "access": "writable",
                    "base_branch": "main",
                    "branch_hint": "benchmark/exact"
                }],
                "work_item": {
                    "role": "engineer",
                    "queue": "code_ready",
                    "kind": "code",
                    "target": "Issue { number: ItemNumber(1) }",
                    "context": "{}"
                },
                "action": "open_pr",
                "correlation_key": "exact-patch-test",
                "checkout": "writable"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.path().join("benchmark.toml"),
            r#"schema = "temper.benchmark.v1"
name = "exact-patch-test"
fixture = "fixture"
workspace_context = "context.json"
capture = "diagnostic"
jig_script = "jig.json"
expected_patch = "expected.patch"
"#,
        )
        .unwrap();

        let manifest = load_benchmark_manifest(root.path().join("benchmark.toml")).unwrap();
        let workspace = prepare_benchmark_workspace(&manifest, 1).unwrap();
        let repository = workspace.root().join("repo");
        fs::write(repository.join("value.txt"), "after\n").unwrap();
        let actual = Command::new("git")
            .args(["diff", "HEAD", "--", "."])
            .current_dir(&repository)
            .output()
            .unwrap();
        fs::write(root.path().join("expected.patch"), actual.stdout).unwrap();

        let passed = validate_expected_patch(&manifest, &workspace, 0)
            .unwrap()
            .unwrap();
        assert_eq!(passed.status, "passed");

        fs::write(repository.join("extra.txt"), "unexpected\n").unwrap();
        let extra = validate_expected_patch(&manifest, &workspace, 1)
            .unwrap()
            .unwrap();
        assert_eq!(extra.status, "failed");
        assert_eq!(extra.untracked_files, 1);

        fs::remove_file(repository.join("extra.txt")).unwrap();
        fs::write(repository.join("value.txt"), "different\n").unwrap();
        let different = validate_expected_patch(&manifest, &workspace, 0)
            .unwrap()
            .unwrap();
        assert_eq!(different.status, "failed");
    }
}
