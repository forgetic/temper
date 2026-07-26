// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::model::{
    ASSERTION_STATUS_FAILED, ASSERTION_STATUS_PASSED, ASSERTION_STATUS_TIMED_OUT,
    AssertionResultEvidence, RunEvidenceArtifact,
};
use super::{SCRIPT_CONTEXT_SCHEMA, SCRIPT_CONTEXT_VERSION, SCRIPT_KIND_COMMAND, ScriptHook};

pub(super) fn run_hook(
    hook: &ScriptHook,
    artifact: &RunEvidenceArtifact,
    artifact_dir: &Path,
    hook_dir: &Path,
) -> Result<AssertionResultEvidence, String> {
    fs::create_dir_all(hook_dir).map_err(|error| {
        format!(
            "create script assertion artifact dir {}: {error}",
            hook_dir.display()
        )
    })?;

    let context_path = hook_dir.join("context.json");
    let stdout_path = hook_dir.join("stdout.log");
    let stderr_path = hook_dir.join("stderr.log");
    let status_path = hook_dir.join("status.txt");

    let context = script_context(artifact, hook, artifact_dir, hook_dir, &context_path);
    let context_json = serde_json::to_string_pretty(&context)
        .map_err(|error| format!("serialize script assertion context: {error}"))?;
    fs::write(&context_path, format!("{context_json}\n")).map_err(|error| {
        format!(
            "write script assertion context {}: {error}",
            context_path.display()
        )
    })?;

    let stdout = File::create(&stdout_path).map_err(|error| {
        format!(
            "create script assertion stdout {}: {error}",
            stdout_path.display()
        )
    })?;
    let stderr = File::create(&stderr_path).map_err(|error| {
        format!(
            "create script assertion stderr {}: {error}",
            stderr_path.display()
        )
    })?;

    let mut command = Command::new(bash_path());
    command
        .arg(&hook.command)
        .arg(&context_path)
        .current_dir(&hook.cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("TEMPER_SCENARIO_CONTEXT", &context_path)
        .env("TEMPER_SCENARIO_ARTIFACT_DIR", hook_dir)
        .env("TEMPER_SCENARIO_RUN_ARTIFACT_DIR", artifact_dir)
        .env("TEMPER_SCENARIO_HOOK_ID", &hook.id)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    for name in &hook.env_allow {
        if let Ok(value) = env::var(name) {
            command.env(name, value);
        }
    }

    let start = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let duration_ms = elapsed_ms(start.elapsed());
            let exit_status = format!("failed to spawn bash hook: {error}");
            fs::write(&status_path, format!("{exit_status}\n")).map_err(|write_error| {
                format!(
                    "write script assertion status {}: {write_error}",
                    status_path.display()
                )
            })?;
            let paths = HookOutputPaths {
                context: &context_path,
                stdout: &stdout_path,
                stderr: &stderr_path,
                status: &status_path,
            };
            return Ok(execution_result(
                hook,
                ASSERTION_STATUS_FAILED,
                "Script assertion hook failed to start.",
                paths,
                exit_status,
                duration_ms,
                vec![format!("spawn error: {error}")],
            ));
        }
    };

    let timeout = Duration::from_millis(hook.timeout_ms);
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let status = child.wait().map_err(|error| {
                    format!(
                        "wait for timed-out script assertion hook `{}`: {error}",
                        hook.id
                    )
                })?;
                break (status, true);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                return Err(format!(
                    "poll script assertion hook `{}` status: {error}",
                    hook.id
                ));
            }
        }
    };

    let duration_ms = elapsed_ms(start.elapsed());
    let exit_status = if timed_out {
        format!(
            "timed out after {}ms; terminated with {}",
            hook.timeout_ms,
            display_exit_status(status)
        )
    } else {
        display_exit_status(status)
    };
    fs::write(&status_path, format!("{exit_status}\n")).map_err(|error| {
        format!(
            "write script assertion status {}: {error}",
            status_path.display()
        )
    })?;

    let mut details = vec![
        format!("command: `{}`", hook.command.display()),
        format!("cwd: `{}`", hook.cwd.display()),
        format!("context: `{}`", context_path.display()),
        format!("stdout: `{}`", stdout_path.display()),
        format!("stderr: `{}`", stderr_path.display()),
        format!("status: `{}`", status_path.display()),
        format!("exit status: {exit_status}"),
        format!("duration: {duration_ms}ms (timeout: {}ms)", hook.timeout_ms),
    ];
    if timed_out {
        details.push(format!("hook timed out after {}ms", hook.timeout_ms));
    } else if !status.success() {
        details.push("hook exited non-zero".to_string());
    }
    if let Some(excerpt) = read_excerpt(&stdout_path) {
        details.push(format!("stdout excerpt: {excerpt}"));
    }
    if let Some(excerpt) = read_excerpt(&stderr_path) {
        details.push(format!("stderr excerpt: {excerpt}"));
    }

    let assertion_status = if timed_out {
        ASSERTION_STATUS_TIMED_OUT
    } else if status.success() {
        ASSERTION_STATUS_PASSED
    } else {
        ASSERTION_STATUS_FAILED
    };
    let description = if assertion_status == ASSERTION_STATUS_PASSED {
        "Script assertion hook completed successfully."
    } else if timed_out {
        "Script assertion hook timed out."
    } else {
        "Script assertion hook exited non-zero."
    };

    let paths = HookOutputPaths {
        context: &context_path,
        stdout: &stdout_path,
        stderr: &stderr_path,
        status: &status_path,
    };
    Ok(execution_result(
        hook,
        assertion_status,
        description,
        paths,
        exit_status,
        duration_ms,
        details,
    ))
}

struct HookOutputPaths<'a> {
    context: &'a Path,
    stdout: &'a Path,
    stderr: &'a Path,
    status: &'a Path,
}

fn execution_result(
    hook: &ScriptHook,
    status: &str,
    description: &str,
    paths: HookOutputPaths<'_>,
    exit_status: String,
    duration_ms: u64,
    details: Vec<String>,
) -> AssertionResultEvidence {
    AssertionResultEvidence {
        id: hook.id.clone(),
        required: hook.required,
        status: status.to_string(),
        description: description.to_string(),
        artifact: Some(format!("script:{}", hook.id)),
        kind: Some(SCRIPT_KIND_COMMAND.to_string()),
        phase: Some(hook.phase.clone()),
        command: Some(hook.command.display().to_string()),
        cwd: Some(hook.cwd.display().to_string()),
        context_path: Some(paths.context.display().to_string()),
        stdout_path: Some(paths.stdout.display().to_string()),
        stderr_path: Some(paths.stderr.display().to_string()),
        status_path: Some(paths.status.display().to_string()),
        exit_status: Some(exit_status),
        timeout_ms: Some(hook.timeout_ms),
        duration_ms: Some(duration_ms),
        details,
    }
}

fn script_context(
    artifact: &RunEvidenceArtifact,
    hook: &ScriptHook,
    artifact_dir: &Path,
    hook_dir: &Path,
    context_path: &Path,
) -> serde_json::Value {
    let provider = artifact.provider.as_ref();
    let first_issue = artifact.final_state.issues.first();
    let first_pull_request = artifact.final_state.pull_requests.first();
    let issue_number = provider
        .and_then(|provider| provider.issue_number)
        .or_else(|| first_issue.map(|issue| issue.number));
    let pr_number = provider
        .and_then(|provider| provider.pr_number)
        .or_else(|| first_pull_request.map(|pull_request| pull_request.number));
    let head_branch = provider
        .and_then(|provider| provider.head_branch.as_deref())
        .or_else(|| {
            first_pull_request.and_then(|pull_request| pull_request.head_branch.as_deref())
        });
    let merged_sha = provider
        .and_then(|provider| provider.merged_sha.as_deref())
        .or_else(|| first_pull_request.and_then(|pull_request| pull_request.merged_sha.as_deref()));

    json!({
        "schema": SCRIPT_CONTEXT_SCHEMA,
        "version": SCRIPT_CONTEXT_VERSION,
        "hook_id": &hook.id,
        "phase": &hook.phase,
        "scenario_path": &artifact.scenario.scenario_path,
        "manifest_path": &artifact.scenario.manifest_path,
        "artifact_directory": hook_dir.display().to_string(),
        "run_artifact_directory": artifact_dir.display().to_string(),
        "context_path": context_path.display().to_string(),
        "runner_id": &artifact.scenario.runner_id,
        "tier": &artifact.scenario.tier,
        "forgejo_url": provider.and_then(|provider| provider.forgejo_url.as_deref()),
        "repo_slug": provider.and_then(|provider| provider.repo_slug.as_deref()),
        "issue_number": issue_number,
        "pr_number": pr_number,
        "head_branch": head_branch,
        "merged_sha": merged_sha,
        "provider": provider,
        "run_evidence": artifact,
    })
}

fn bash_path() -> PathBuf {
    ["/usr/bin/bash", "/bin/bash"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("bash"))
}

fn display_exit_status(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_string(),
    }
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn read_excerpt(path: &Path) -> Option<String> {
    let source = fs::read_to_string(path).ok()?;
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(truncate_line)
        .collect::<Vec<_>>();
    if source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > lines.len()
    {
        lines.push("...".to_string());
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" | "))
    }
}

fn truncate_line(line: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut chars = line.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
