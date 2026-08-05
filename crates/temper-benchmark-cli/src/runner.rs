// SPDX-License-Identifier: MPL-2.0

//! Direct, deterministic agent-session benchmark execution.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use jig_core::ScriptFile;
use jig_server::FakeLlm;
use serde::Serialize;
use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1};
use temper_protocol_agent::PROVIDER_CREDENTIALS_ENV;
use temper_worker::{
    AgentRunOutput, AgentRunner, AgentRuntimeLimitsV1, OutOfProcessRunner, TraceCollector,
    WorkerAgentTraceConfig,
};

use crate::{
    AggregateError, AnalyzeOptions, ArtifactLayoutError, BenchmarkAggregateV1,
    BenchmarkArtifactLayout, BenchmarkModeV1, BenchmarkRunV1, ReportWriteError,
    ResolvedBenchmarkManifest, TraceIngestError, TraceInputKindV1, WorkspacePreparationError,
    aggregate_run_summaries, analyze_trace, collect_environment_metadata, load_benchmark_manifest,
    prepare_benchmark_workspace, render_aggregate_markdown, write_canonical_export,
    write_run_summary,
};

mod diff;
mod live;
mod redaction;
mod validation;

use diff::collect_diff_artifact;
pub use diff::{
    DIFF_ARTIFACT_VERSION, DiffArtifactV1, DiffFileEvidenceV1, RepositoryDiffEvidenceV1,
};
pub use live::{LIVE_OPT_IN_ENV, LiveRunOptions, run_live};
use redaction::SecretRedactor;
pub use validation::{
    AcceptedSubmitEvidenceV1, VALIDATION_ARTIFACT_VERSION, ValidationArtifactV1,
    ValidationCommandEvidenceV1,
};
use validation::{accepted_submit_evidence, run_post_run_commands, validation_summary};

const DUMMY_PROVIDER_CREDENTIAL: &str =
    r#"{"type":"api-key","api_key":"temper-benchmark-harness-dummy"}"#;
const HARNESS_MODEL: &str = "temper-benchmark-harness";

/// Options for the CI-safe direct harness runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessRunOptions {
    pub benchmark: PathBuf,
    pub agent_bin: PathBuf,
    pub output_dir: PathBuf,
    /// `None` uses the manifest default.
    pub repetitions: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum BenchmarkRunError {
    #[error(transparent)]
    Config(#[from] temper_config::ConfigError),
    #[error(transparent)]
    Manifest(#[from] crate::BenchmarkManifestError),
    #[error(transparent)]
    Artifacts(#[from] ArtifactLayoutError),
    #[error(transparent)]
    Workspace(#[from] WorkspacePreparationError),
    #[error(transparent)]
    Trace(#[from] TraceIngestError),
    #[error("cannot recover worker-owned trace spool: {0}")]
    WorkerTrace(#[source] temper_worker::TraceError),
    #[error(transparent)]
    Report(#[from] ReportWriteError),
    #[error(transparent)]
    Aggregate(#[from] AggregateError),
    #[error("invalid benchmark run configuration: {0}")]
    Invalid(String),
    #[error(
        "live benchmark execution requires `{LIVE_OPT_IN_ENV}=1`; no config, credentials, workspace, or provider was accessed"
    )]
    LiveOptInRequired,
    #[error("live benchmark configuration is incompatible: {0}")]
    LiveConfiguration(String),
    #[error(
        "live benchmarks require first-party Temper supervision; selected agent invocation is third-party"
    )]
    ThirdPartySupervision,
    #[error(
        "refusing to write {artifact}: resolved provider credentials appeared in artifact content"
    )]
    SecretArtifact { artifact: &'static str },
    #[error("cannot deserialize redacted {artifact}: {source}")]
    RedactedJson {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot load Jig script `{path}`: {message}")]
    JigScript { path: PathBuf, message: String },
    #[error("cannot start Jig provider: {0}")]
    JigServer(#[source] io::Error),
    #[error("agent repetition {repetition} failed: {message}")]
    Agent { repetition: u32, message: String },
    #[error("trace collector produced {actual} runs for repetition {repetition}; expected one")]
    TraceRunCount { repetition: u32, actual: usize },
    #[error("cannot fingerprint repetition {repetition} after the agent session: {message}")]
    Fingerprint { repetition: u32, message: String },
    #[error("git command `{command}` failed in `{cwd}` ({status}): {stderr}")]
    Git {
        command: String,
        cwd: PathBuf,
        status: String,
        stderr: String,
    },
    #[error("cannot {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot serialize {artifact}: {source}")]
    Json {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

/// Runs every harness repetition in a fresh prepared workspace and writes all
/// repetition and aggregate artifacts. Agent-process failures are returned only
/// after every independent repetition has been attempted and persisted.
pub fn run_harness(options: &HarnessRunOptions) -> Result<BenchmarkAggregateV1, BenchmarkRunError> {
    let manifest = load_benchmark_manifest(&options.benchmark)?;
    if manifest.manifest().capture == CaptureModeV1::Off {
        return Err(BenchmarkRunError::Invalid(
            "harness mode requires capture other than `off`".to_string(),
        ));
    }
    let agent_bin = validate_agent_binary(&options.agent_bin)?;
    let repetitions = options
        .repetitions
        .unwrap_or(manifest.manifest().repetitions);
    if repetitions == 0 {
        return Err(BenchmarkRunError::Invalid(
            "repetitions must be at least one".to_string(),
        ));
    }
    reject_output_inside_fixture(&options.output_dir, manifest.fixture_dir())?;

    let layout = BenchmarkArtifactLayout::create(&options.output_dir, repetitions)?;
    let mut summaries = Vec::with_capacity(repetitions as usize);
    let mut first_agent_failure = None;
    for repetition in 1..=repetitions {
        let completed = run_repetition(&manifest, &layout, &agent_bin, repetition)?;
        summaries.push(completed.summary);
        if first_agent_failure.is_none() {
            first_agent_failure = completed.agent_failure;
        }
    }

    let aggregate = aggregate_run_summaries(summaries)?;
    write_json(&layout.aggregate_json, &aggregate, "benchmark aggregate")?;
    write_bytes(
        &layout.aggregate_markdown,
        render_aggregate_markdown(&aggregate).as_bytes(),
        "write benchmark aggregate Markdown",
    )?;
    match first_agent_failure {
        Some(error) => Err(error),
        None => Ok(aggregate),
    }
}

fn reject_output_inside_fixture(
    output_dir: &Path,
    fixture_dir: &Path,
) -> Result<(), BenchmarkRunError> {
    let output = projected_absolute_path(output_dir)?;
    if output.starts_with(fixture_dir) {
        return Err(BenchmarkRunError::Invalid(format!(
            "output directory `{}` must not be inside fixture `{}`",
            output.display(),
            fixture_dir.display()
        )));
    }
    Ok(())
}

/// Resolves every existing prefix (including links) while retaining a safe
/// projection for suffixes which the artifact layout has not created yet.
fn projected_absolute_path(path: &Path) -> Result<PathBuf, BenchmarkRunError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|source| BenchmarkRunError::Io {
            operation: "resolve current directory",
            path: PathBuf::from("."),
            source,
        })?;
        fs::canonicalize(&cwd)
            .map_err(|source| BenchmarkRunError::Io {
                operation: "resolve current directory",
                path: cwd,
                source,
            })?
            .join(path)
    };

    let mut projected = PathBuf::new();
    let mut unresolved_depth = 0_u64;
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => projected.push(prefix.as_os_str()),
            Component::RootDir => projected.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                projected.pop();
                unresolved_depth = unresolved_depth.saturating_sub(1);
            }
            Component::Normal(value) => {
                projected.push(value);
                if unresolved_depth > 0 {
                    unresolved_depth = unresolved_depth.saturating_add(1);
                    continue;
                }
                match fs::canonicalize(&projected) {
                    Ok(resolved) => projected = resolved,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => unresolved_depth = 1,
                    Err(source) => {
                        return Err(BenchmarkRunError::Io {
                            operation: "resolve output directory prefix",
                            path: projected,
                            source,
                        });
                    }
                }
            }
        }
    }
    Ok(projected)
}

fn validate_agent_binary(path: &Path) -> Result<PathBuf, BenchmarkRunError> {
    let metadata = fs::metadata(path).map_err(|source| BenchmarkRunError::Io {
        operation: "inspect agent binary",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(BenchmarkRunError::Invalid(format!(
            "agent binary `{}` is not a regular file",
            path.display()
        )));
    }
    let path = fs::canonicalize(path).map_err(|source| BenchmarkRunError::Io {
        operation: "resolve agent binary",
        path: path.to_path_buf(),
        source,
    })?;
    if path.to_str().is_none() {
        return Err(BenchmarkRunError::Invalid(
            "agent binary path is not valid UTF-8".to_string(),
        ));
    }
    Ok(path)
}

pub(super) struct CompletedRepetition {
    pub(super) summary: crate::RunSummaryV1,
    pub(super) agent_failure: Option<BenchmarkRunError>,
}

fn run_repetition(
    manifest: &ResolvedBenchmarkManifest,
    layout: &BenchmarkArtifactLayout,
    agent_bin: &Path,
    repetition: u32,
) -> Result<CompletedRepetition, BenchmarkRunError> {
    let workspace = prepare_benchmark_workspace(manifest, repetition)?;
    let paths = layout.snapshot_inputs(repetition, manifest, &workspace)?;

    let script = ScriptFile::load(manifest.jig_script_path()).map_err(|error| {
        BenchmarkRunError::JigScript {
            path: manifest.jig_script_path().to_path_buf(),
            message: error.to_string(),
        }
    })?;
    let jig = FakeLlm::start(script.into_script()).map_err(BenchmarkRunError::JigServer)?;

    let policy = AgentActivityCapturePolicyV1 {
        capture: manifest.manifest().capture,
        ..AgentActivityCapturePolicyV1::default()
    };
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: policy.clone(),
        spool_root: Some(workspace.temporary_root().join("trace-spool")),
    });
    let command = vec![
        agent_bin
            .to_str()
            .expect("validated UTF-8 agent path")
            .to_string(),
        "--provider".to_string(),
        "deepseek".to_string(),
        "--model".to_string(),
        HARNESS_MODEL.to_string(),
        "--provider-url".to_string(),
        jig.base_url(),
        "--subagents".to_string(),
        "off".to_string(),
    ];
    let runner = OutOfProcessRunner::new(command)
        .with_env(vec![(
            PROVIDER_CREDENTIALS_ENV.to_string(),
            DUMMY_PROVIDER_CREDENTIAL.to_string(),
        )])
        .with_runtime_limits(Some(AgentRuntimeLimitsV1::default()))
        .with_trace_policy(Some(policy))
        .with_shared_trace_collector(collector.clone());
    let context = workspace.context().clone();
    let cwd = workspace.root().to_path_buf();
    let job_id = format!("benchmark-repetition-{repetition:03}");
    let outcome =
        temper_worker_io::block_on(async move { runner.run(&job_id, &context, &cwd).await });
    drop(jig);
    let (output, agent_failure) = match outcome {
        Ok(output) => (Some(output), None),
        Err(error) => (
            None,
            Some(BenchmarkRunError::Agent {
                repetition,
                message: format!("{:?}: {}", error.class, error.message),
            }),
        ),
    };

    let summary = finalize_repetition(
        manifest,
        &paths,
        &workspace,
        collector,
        output,
        BenchmarkModeV1::Harness,
        repetition,
        None,
    )?;
    Ok(CompletedRepetition {
        summary,
        agent_failure,
    })
}

#[allow(clippy::too_many_arguments)]
fn finalize_repetition(
    manifest: &ResolvedBenchmarkManifest,
    paths: &crate::RepetitionArtifactPaths,
    workspace: &crate::PreparedBenchmarkWorkspace,
    collector: TraceCollector,
    mut output: Option<AgentRunOutput>,
    mode: BenchmarkModeV1,
    repetition: u32,
    redactor: Option<&SecretRedactor>,
) -> Result<crate::RunSummaryV1, BenchmarkRunError> {
    if let (Some(redactor), Some(output)) = (redactor, output.as_mut()) {
        output.result = redactor.redacted(&output.result, "workspace result")?;
    }

    workspace.verify_context_directories()?;
    let accepted_submit = accepted_submit_evidence(
        output
            .as_ref()
            .and_then(|output| output.accepted_submit.as_ref()),
        workspace,
        repetition,
    )?;
    let post_run_commands = run_post_run_commands(manifest, workspace);
    workspace.verify_context_directories()?;
    let mut diff = collect_diff_artifact(workspace)?;
    let mut validation = ValidationArtifactV1 {
        version: VALIDATION_ARTIFACT_VERSION,
        accepted_submit,
        post_run_commands,
    };
    if let Some(redactor) = redactor {
        validation = redactor.redacted(&validation, "validation evidence")?;
        diff = redactor.redacted(&diff, "diff evidence")?;
    }

    let mut recovered = collector
        .recover()
        .map_err(BenchmarkRunError::WorkerTrace)?;
    if recovered.len() != 1 {
        return Err(BenchmarkRunError::TraceRunCount {
            repetition,
            actual: recovered.len(),
        });
    }
    let recovered = recovered.pop().expect("one recovered trace");
    let mut trace = crate::ingest::normalize_worker_trace(
        TraceInputKindV1::JournalDirectory,
        recovered.events,
        recovered.blobs,
    )?;
    if let Some(redactor) = redactor {
        redactor.ensure_safe_attachments(&trace.attachments)?;
        trace.events = redactor.redacted(&trace.events, "canonical trace events")?;
    }
    let prefixes = manifest
        .manifest()
        .validation_command_prefixes
        .iter()
        .map(|argv| argv.join(" "))
        .collect();
    let discovery_prefixes = manifest
        .manifest()
        .discovery_command_prefixes
        .iter()
        .map(|argv| argv.join(" "))
        .collect();
    let mut summary = analyze_trace(
        &trace,
        &AnalyzeOptions {
            validation_command_prefixes: prefixes,
            discovery_command_prefixes: discovery_prefixes,
            graph_decision_targets: manifest.manifest().graph_decision_targets.clone(),
        },
    );
    summary.benchmark = Some(BenchmarkRunV1 {
        name: manifest.manifest().name.clone(),
        mode,
        repetition,
    });
    summary.host = Some(collect_environment_metadata(
        &trace.events,
        &manifest.manifest().annotations,
    ));
    summary.validation = Some(validation_summary(&validation));
    summary.diff = Some(diff.statistics.clone());
    summary.workspace_result = output.as_ref().map(|output| output.result.clone());
    if let Some(redactor) = redactor {
        summary = redactor.redacted(&summary, "run summary")?;
    }

    if let Some(redactor) = redactor {
        let canonical = trace.canonical_export()?;
        redactor.ensure_safe_bytes(&canonical, "canonical trace export")?;
        write_bytes(
            &paths.canonical_trace,
            &canonical,
            "write canonical trace export",
        )?;
        if let Some(output) = &output {
            ensure_serialized_safe(redactor, &output.result, "workspace result")?;
        }
        ensure_serialized_safe(redactor, &validation, "validation evidence")?;
        ensure_serialized_safe(redactor, &diff, "diff evidence")?;
        ensure_serialized_safe(redactor, &summary, "run summary")?;
        let markdown = crate::render_run_summary_markdown(&summary);
        redactor.ensure_safe_bytes(markdown.as_bytes(), "run summary Markdown")?;
    } else {
        write_canonical_export(&trace, &paths.canonical_trace)?;
    }
    if let Some(output) = &output {
        write_json(&paths.workspace_result, &output.result, "workspace result")?;
    } else {
        remove_optional_artifact(&paths.workspace_result)?;
    }
    write_json(
        &paths.validation_evidence,
        &validation,
        "validation evidence",
    )?;
    write_json(&paths.diff_statistics, &diff, "diff evidence")?;
    write_run_summary(&summary, &paths.root)?;
    Ok(summary)
}

fn remove_optional_artifact(path: &Path) -> Result<(), BenchmarkRunError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(BenchmarkRunError::Io {
            operation: "remove unavailable optional artifact",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_serialized_safe<T: Serialize>(
    redactor: &SecretRedactor,
    value: &T,
    artifact: &'static str,
) -> Result<(), BenchmarkRunError> {
    let bytes =
        serde_json::to_vec(value).map_err(|source| BenchmarkRunError::Json { artifact, source })?;
    redactor.ensure_safe_bytes(&bytes, artifact)
}

fn write_json<T: Serialize>(
    path: &Path,
    value: &T,
    artifact: &'static str,
) -> Result<(), BenchmarkRunError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| BenchmarkRunError::Json { artifact, source })?;
    bytes.push(b'\n');
    write_bytes(path, &bytes, "write JSON artifact")
}

fn write_bytes(
    path: &Path,
    bytes: &[u8],
    operation: &'static str,
) -> Result<(), BenchmarkRunError> {
    fs::write(path, bytes).map_err(|source| BenchmarkRunError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}
