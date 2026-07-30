// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use temper_testing::live_manifest::LiveManifestEvidence;

#[derive(Debug)]
pub(super) struct RetainedLogPaths {
    pub(super) workspace_root: PathBuf,
    pub(super) init_log: PathBuf,
    pub(super) repo_populate_log: PathBuf,
    pub(super) standalone_log: PathBuf,
    pub(super) fake_llm_log: PathBuf,
    pub(super) ci_diagnostics_log: PathBuf,
    pub(super) codebase_mcp_log: Option<PathBuf>,
    pub(super) stimulus_logs: Vec<PathBuf>,
}

pub(super) fn copy_report_artifacts(
    evidence: &LiveManifestEvidence,
    artifact_dir: &Path,
) -> Result<RetainedLogPaths, String> {
    let root = artifact_dir.join("live-manifest-artifacts");
    fs::create_dir_all(&root)
        .map_err(|error| format!("create live artifact directory {}: {error}", root.display()))?;
    let stimulus_logs = copy_stimulus_logs(&evidence.logs.standalone_log, &root)?;
    let retained = RetainedLogPaths {
        workspace_root: root.clone(),
        init_log: root.join("init.log"),
        repo_populate_log: root.join("repo-populate.log"),
        standalone_log: root.join("standalone.log"),
        fake_llm_log: root.join("fake-llm.log"),
        ci_diagnostics_log: root.join("ci-diagnostics.log"),
        codebase_mcp_log: evidence
            .codebase_memory
            .as_ref()
            .map(|_| root.join("fake-codebase-memory-mcp.jsonl")),
        stimulus_logs,
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
    if let (Some(codebase_memory), Some(destination)) = (
        evidence.codebase_memory.as_ref(),
        retained.codebase_mcp_log.as_ref(),
    ) {
        copy_log(&codebase_memory.fake_mcp_log, destination)?;
    }
    Ok(retained)
}

fn copy_stimulus_logs(
    standalone_log: &Path,
    destination_root: &Path,
) -> Result<Vec<PathBuf>, String> {
    let sources = stimulus_log_paths(standalone_log)?;
    let mut retained = Vec::with_capacity(sources.len());
    for source in sources {
        let destination = destination_root.join(
            source
                .file_name()
                .expect("stimulus log source has a file name"),
        );
        copy_log(&source, &destination)?;
        retained.push(destination);
    }
    Ok(retained)
}

pub(super) fn stimulus_log_paths(standalone_log: &Path) -> Result<Vec<PathBuf>, String> {
    let Some(log_dir) = standalone_log.parent() else {
        return Ok(Vec::new());
    };
    let mut sources = fs::read_dir(log_dir)
        .map_err(|error| format!("read live log directory {}: {error}", log_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    (name.starts_with("standalone.before-restart-")
                        || name.starts_with("standalone.stimulus-"))
                        && name.ends_with(".log")
                })
        })
        .collect::<Vec<_>>();
    sources.sort();
    Ok(sources)
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
