// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use temper_scenario_core::load_resolved_manifest_toml;
use temper_testing::live_basic_delivery::{
    LiveBasicDeliveryEvidence, ScenarioBundle, TemperCommand, run_live_basic_delivery,
};

use crate::run_context::ScenarioRunFacts;
use crate::run_evidence;

const PRIMARY_TEMPER_BIN_ENV: &str = "TEMPER_SCENARIO_TEMPER_BIN";
const COMPAT_TEMPER_BIN_ENV: &str = "TEMPER_BIN";

pub(super) fn run_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let evidence = run_live(scenario_path, manifest_path, temper_bin)?;
    let lines = live_evidence_lines(&evidence, None);
    let artifact = live_artifact(&evidence, context, &lines, None);
    print_outcome(&lines, facts);
    retain_artifact_workspace(evidence);
    Ok(artifact)
}

pub(super) fn evidence_lines(
    scenario_path: &Path,
    manifest_path: &Path,
    temper_bin: Option<&Path>,
    artifact_dir: Option<&Path>,
) -> Result<Vec<String>, String> {
    let evidence = run_live(scenario_path, manifest_path, temper_bin)?;
    if let Some(artifact_dir) = artifact_dir {
        let retained_logs = copy_report_artifacts(&evidence, artifact_dir)?;
        Ok(live_evidence_lines(&evidence, Some(&retained_logs)))
    } else {
        let lines = live_evidence_lines(&evidence, None);
        retain_artifact_workspace(evidence);
        Ok(lines)
    }
}

fn run_live(
    scenario_path: &Path,
    manifest_path: &Path,
    temper_bin: Option<&Path>,
) -> Result<LiveBasicDeliveryEvidence, String> {
    let manifest = load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())?;
    let scenario = ScenarioBundle::from_manifest(
        scenario_path.to_path_buf(),
        manifest_path.to_path_buf(),
        manifest,
    )?;
    let temper = resolve_temper_command(temper_bin)?;
    run_live_basic_delivery(scenario, temper)
}

fn print_outcome(lines: &[String], facts: &ScenarioRunFacts) {
    println!("scenario: {}", super::SCENARIO_NAME);
    facts.print_stdout();
    println!("verdict: passed");
    println!("evidence:");
    for line in lines {
        println!("  {line}");
    }
}

fn retain_artifact_workspace(evidence: LiveBasicDeliveryEvidence) {
    // Live CLI output cites files under the harness workspace. The harness
    // normally deletes that temp tree when evidence is dropped (which is ideal
    // for ignored tests), but direct operator runs need the printed log/artifact
    // paths to remain readable after the CLI exits. Validation reports use
    // copy_report_artifacts instead, so their uploaded artifact directory owns
    // the cited logs and can let the temp workspace drop normally.
    std::mem::forget(evidence);
}

#[derive(Debug)]
struct RetainedLogPaths {
    workspace_root: PathBuf,
    init_log: PathBuf,
    repo_populate_log: PathBuf,
    standalone_log: PathBuf,
    fake_llm_log: PathBuf,
    ci_diagnostics_log: PathBuf,
}

fn copy_report_artifacts(
    evidence: &LiveBasicDeliveryEvidence,
    artifact_dir: &Path,
) -> Result<RetainedLogPaths, String> {
    let root = artifact_dir.join("live-basic-delivery-artifacts");
    fs::create_dir_all(&root)
        .map_err(|error| format!("create live artifact directory {}: {error}", root.display()))?;
    let retained = RetainedLogPaths {
        workspace_root: root.clone(),
        init_log: root.join("init.log"),
        repo_populate_log: root.join("repo-populate.log"),
        standalone_log: root.join("standalone.log"),
        fake_llm_log: root.join("fake-llm.log"),
        ci_diagnostics_log: root.join("ci-diagnostics.log"),
    };
    copy_log(&evidence.logs.init_log, &retained.init_log)?;
    copy_log(
        &evidence.logs.repo_populate_log,
        &retained.repo_populate_log,
    )?;
    copy_log(&evidence.logs.standalone_log, &retained.standalone_log)?;
    copy_log(&evidence.logs.fake_llm_log, &retained.fake_llm_log)?;
    copy_log(
        &evidence.logs.ci_diagnostics_log,
        &retained.ci_diagnostics_log,
    )?;
    Ok(retained)
}

fn copy_log(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination).map(|_| ()).map_err(|error| {
        format!(
            "copy live artifact {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

fn live_artifact(
    evidence: &LiveBasicDeliveryEvidence,
    context: &run_evidence::RunEvidenceContext,
    lines: &[String],
    retained_logs: Option<&RetainedLogPaths>,
) -> run_evidence::RunEvidenceArtifact {
    let workspace_root = retained_logs
        .map(|logs| &logs.workspace_root)
        .unwrap_or(&evidence.logs.workspace_root);
    let init_log = retained_logs
        .map(|logs| &logs.init_log)
        .unwrap_or(&evidence.logs.init_log);
    let repo_populate_log = retained_logs
        .map(|logs| &logs.repo_populate_log)
        .unwrap_or(&evidence.logs.repo_populate_log);
    let standalone_log = retained_logs
        .map(|logs| &logs.standalone_log)
        .unwrap_or(&evidence.logs.standalone_log);
    let fake_llm_log = retained_logs
        .map(|logs| &logs.fake_llm_log)
        .unwrap_or(&evidence.logs.fake_llm_log);
    let ci_diagnostics_log = retained_logs
        .map(|logs| &logs.ci_diagnostics_log)
        .unwrap_or(&evidence.logs.ci_diagnostics_log);

    let mut artifact = context.artifact(run_evidence::FinalStateEvidence {
        issues: vec![run_evidence::IssueStateEvidence {
            number: evidence.final_state.issue.number,
            id: Some("intake".to_string()),
            title: Some(evidence.final_state.issue.title.clone()),
            state: Some(evidence.final_state.issue.state.clone()),
            labels: evidence.final_state.issue.labels.clone(),
        }],
        pull_requests: vec![run_evidence::PullRequestStateEvidence {
            number: evidence.final_state.pull_request.number,
            id: Some("implementation".to_string()),
            title: Some(evidence.final_state.pull_request.title.clone()),
            state: Some(evidence.final_state.pull_request.state.clone()),
            labels: evidence.final_state.pull_request.labels.clone(),
            head_branch: Some(evidence.final_state.pull_request.head_branch.clone()),
            head_sha: evidence.final_state.pull_request.head_sha.clone(),
            merged_sha: evidence.final_state.pull_request.head_sha.clone(),
        }],
        ci: run_evidence::CiStateEvidence {
            completed_jobs: Some(evidence.final_state.ci_jobs.len()),
            jobs: evidence
                .final_state
                .ci_jobs
                .iter()
                .map(|job| run_evidence::CiJobEvidence {
                    name: job.name.clone(),
                    status: job.status.clone(),
                    pull_request_number: Some(evidence.final_state.pull_request.number),
                    conclusion: job.conclusion.clone(),
                    url: job.url.clone(),
                })
                .collect(),
        },
    });
    artifact.convergence = Some(run_evidence::ConvergenceEvidence {
        startup_ms: Some(duration_ms(evidence.startup)),
        convergence_ms: Some(duration_ms(evidence.convergence)),
        total_elapsed_ms: Some(duration_ms(evidence.total_elapsed)),
        poll_backstop_ms: Some(duration_ms(evidence.poll_backstop)),
        ..run_evidence::ConvergenceEvidence::default()
    });
    artifact.provider = Some(run_evidence::ProviderEvidence {
        forgejo_url: Some(evidence.forge_url.clone()),
        repo_slug: Some(evidence.repo_slug.clone()),
        issue_number: Some(evidence.final_state.issue.number),
        pr_number: Some(evidence.final_state.pull_request.number),
        head_branch: Some(evidence.final_state.pull_request.head_branch.clone()),
        merged_sha: evidence.final_state.pull_request.head_sha.clone(),
        temper_binary: Some(evidence.temper_binary.display().to_string()),
        fake_llm_url: Some(evidence.fake_llm.base_url.clone()),
    });
    artifact.artifacts = run_evidence::ArtifactCollections {
        log_paths: vec![
            init_log.display().to_string(),
            repo_populate_log.display().to_string(),
            standalone_log.display().to_string(),
            fake_llm_log.display().to_string(),
        ],
        artifact_paths: vec![
            workspace_root.display().to_string(),
            ci_diagnostics_log.display().to_string(),
        ],
    };
    artifact.evidence_lines = lines.to_vec();
    artifact
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn live_evidence_lines(
    evidence: &LiveBasicDeliveryEvidence,
    retained_logs: Option<&RetainedLogPaths>,
) -> Vec<String> {
    let workspace_root = retained_logs
        .map(|logs| &logs.workspace_root)
        .unwrap_or(&evidence.logs.workspace_root);
    let init_log = retained_logs
        .map(|logs| &logs.init_log)
        .unwrap_or(&evidence.logs.init_log);
    let repo_populate_log = retained_logs
        .map(|logs| &logs.repo_populate_log)
        .unwrap_or(&evidence.logs.repo_populate_log);
    let standalone_log = retained_logs
        .map(|logs| &logs.standalone_log)
        .unwrap_or(&evidence.logs.standalone_log);
    let fake_llm_log = retained_logs
        .map(|logs| &logs.fake_llm_log)
        .unwrap_or(&evidence.logs.fake_llm_log);
    let ci_diagnostics_log = retained_logs
        .map(|logs| &logs.ci_diagnostics_log)
        .unwrap_or(&evidence.logs.ci_diagnostics_log);
    let mut lines = vec![
        format!("Forgejo URL: {}", evidence.forge_url),
        format!(
            "live topology: repo={} forge_cache_hit={} runner_running={}",
            evidence.repo_slug, evidence.forge_cache_hit, evidence.runner_running
        ),
        format!("temper binary: {}", evidence.temper_binary.display()),
        format!(
            "source issue: #{} \"{}\" state={} labels={:?}",
            evidence.final_state.issue.number,
            evidence.final_state.issue.title,
            evidence.final_state.issue.state,
            evidence.final_state.issue.labels
        ),
        format!(
            "implementation PR: #{} \"{}\" state={} author={} merged_by={:?} head={} sha={:?} labels={:?}",
            evidence.final_state.pull_request.number,
            evidence.final_state.pull_request.title,
            evidence.final_state.pull_request.state,
            evidence.final_state.pull_request.author,
            evidence.final_state.pull_request.merged_by,
            evidence.final_state.pull_request.head_branch,
            evidence.final_state.pull_request.head_sha,
            evidence.final_state.pull_request.labels
        ),
        format!(
            "convergence: {:?} before poll_backstop {:?} (startup {:?}, total {:?})",
            evidence.convergence, evidence.poll_backstop, evidence.startup, evidence.total_elapsed
        ),
        format!(
            "fake LLM: url={} architect_requests={} engineer_requests={} log={}",
            evidence.fake_llm.base_url,
            evidence.fake_llm.architect_requests,
            evidence.fake_llm.engineer_requests,
            fake_llm_log.display()
        ),
    ];
    lines.push(format!(
        "CI jobs: {} completed job(s)",
        evidence.final_state.ci_jobs.len()
    ));
    for job in &evidence.final_state.ci_jobs {
        lines.push(format!(
            "CI job: name={} status={} conclusion={:?} url={:?}",
            job.name, job.status, job.conclusion, job.url
        ));
    }
    lines.extend([
        format!("log/artifact workspace: {}", workspace_root.display()),
        format!("log init: {}", init_log.display()),
        format!("log repo_populate: {}", repo_populate_log.display()),
        format!("log standalone: {}", standalone_log.display()),
        format!("log fake_llm: {}", fake_llm_log.display()),
        format!("artifact CI diagnostics: {}", ci_diagnostics_log.display()),
    ]);
    lines
}

fn resolve_temper_command(explicit: Option<&Path>) -> Result<TemperCommand, String> {
    let binary = if let Some(path) = explicit {
        validate_temper_binary(path, "--temper-bin")?
    } else if let Some(raw) = env::var_os(PRIMARY_TEMPER_BIN_ENV) {
        validate_env_temper_binary(PRIMARY_TEMPER_BIN_ENV, raw)?
    } else if let Some(raw) = env::var_os(COMPAT_TEMPER_BIN_ENV) {
        validate_env_temper_binary(COMPAT_TEMPER_BIN_ENV, raw)?
    } else {
        find_fallback_temper_binary()?
    };
    Ok(TemperCommand::new(binary))
}

fn validate_env_temper_binary(name: &str, raw: std::ffi::OsString) -> Result<PathBuf, String> {
    if raw.as_os_str().is_empty() {
        return Err(format!(
            "{name} is set but empty; pass --temper-bin <PATH> or unset {name}"
        ));
    }
    validate_temper_binary(Path::new(&raw), name)
}

fn validate_temper_binary(path: &Path, source: &str) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!(
            "live basic-delivery {source} path does not exist: {}",
            path.display()
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "live basic-delivery {source} path is not a file: {}",
            path.display()
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        format!(
            "canonicalize live basic-delivery {source} path {}: {error}",
            path.display()
        )
    })
}

fn find_fallback_temper_binary() -> Result<PathBuf, String> {
    let candidates = fallback_candidates();
    for candidate in &candidates {
        if candidate.is_file() {
            return fs::canonicalize(candidate).map_err(|error| {
                format!(
                    "canonicalize fallback temper binary {}: {error}",
                    candidate.display()
                )
            });
        }
    }
    let checked = candidates
        .iter()
        .map(|path| format!("  - {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "could not resolve a standalone `temper` binary for live basic-delivery; pass --temper-bin <PATH>, set {PRIMARY_TEMPER_BIN_ENV}, or run `cargo dev-scenario-run`. Checked fallback candidates:\n{checked}"
    ))
}

fn fallback_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            push_temper_in_dir(&mut candidates, dir);
            if dir
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("deps"))
            {
                if let Some(parent) = dir.parent() {
                    push_temper_in_dir(&mut candidates, parent);
                }
            }
        }
    }
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        push_temper_in_dir(&mut candidates, &target_dir.join("debug"));
        push_temper_in_dir(&mut candidates, &target_dir.join("release"));
    }
    if let Ok(current_dir) = env::current_dir() {
        push_temper_in_dir(&mut candidates, &current_dir.join("target/debug"));
        push_temper_in_dir(&mut candidates, &current_dir.join("target/release"));
        if let Some(root) = find_repo_root(&current_dir) {
            push_temper_in_dir(&mut candidates, &root.join("target/debug"));
            push_temper_in_dir(&mut candidates, &root.join("target/release"));
        }
    }
    candidates
}

fn push_temper_in_dir(candidates: &mut Vec<PathBuf>, dir: &Path) {
    push_unique(
        candidates,
        dir.join(format!("temper{}", env::consts::EXE_SUFFIX)),
    );
}

fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    loop {
        if current.join("Cargo.toml").is_file() && current.join("scenarios").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
