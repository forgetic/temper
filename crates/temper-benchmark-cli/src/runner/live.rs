// SPDX-License-Identifier: MPL-2.0

//! Credential-gated live agent-session execution.

use std::path::PathBuf;

use temper_config::{EnvLookup, EnvMap, LoadInputs, PathResolver};
use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1};
use temper_worker::{
    AgentRunner, AgentRuntimeLimitsV1, AgentToolConfig, OutOfProcessRunner, TraceCollector,
    WorkerAgentTraceConfig, WorkerLivenessLimits,
};
use temper_worker_service::{
    AgentSupervisionKind, agent_invocation_with_first_party_program, worker_liveness_limits,
};

use super::redaction::SecretRedactor;
use super::{
    BenchmarkRunError, CompletedRepetition, finalize_repetition, reject_output_inside_fixture,
    validate_agent_binary, write_bytes, write_json,
};
use crate::{
    BenchmarkAggregateV1, BenchmarkArtifactLayout, BenchmarkConditionV1, BenchmarkModeV1,
    ResolvedBenchmarkManifest, aggregate_run_summaries, load_benchmark_manifest,
    prepare_benchmark_workspace, render_aggregate_markdown,
};

/// Deliberate process-level opt-in required before any live input is touched.
pub const LIVE_OPT_IN_ENV: &str = "TEMPER_BENCHMARK_LIVE";

/// Options for a credential-backed direct live runner.
#[derive(Clone, Debug)]
pub struct LiveRunOptions {
    pub benchmark: PathBuf,
    pub agent_bin: PathBuf,
    pub output_dir: PathBuf,
    /// `None` uses the manifest default.
    pub repetitions: Option<u32>,
    /// Normal Temper config and secret-source overrides.
    pub config: Option<PathBuf>,
    pub credentials: Option<PathBuf>,
    /// Target-era worker pool whose agent profile should shape the invocation.
    pub worker_pool: Option<String>,
    /// Required when the manifest declares a controlled condition profile.
    pub condition: Option<BenchmarkConditionV1>,
}

/// Executes credential-backed live repetitions after an explicit opt-in gate.
/// Agent-process failures are returned only after every independent repetition
/// has been attempted and its redacted artifacts have been persisted.
///
/// The gate is intentionally the first operation: callers can inject an
/// in-memory environment and prove that a rejected request never reads config,
/// credentials, benchmark inputs, or creates a workspace.
pub fn run_live(
    options: &LiveRunOptions,
    env: &EnvMap,
) -> Result<BenchmarkAggregateV1, BenchmarkRunError> {
    if env.get(LIVE_OPT_IN_ENV).as_deref() != Some("1") {
        return Err(BenchmarkRunError::LiveOptInRequired);
    }

    let discovery = PathResolver::from_env(env);
    let (mut resolved, _) = temper_config::load_explicit(&LoadInputs {
        explicit_config: options.config.clone(),
        explicit_credentials: options.credentials.clone(),
        env,
        paths: &discovery,
    })?;
    select_live_worker_pool(&mut resolved, options.worker_pool.as_deref())?;

    // The benchmark owns an isolated per-repetition spool. Supplying a marker
    // here asks the production adapter for the configured (rather than
    // storage-degraded) policy; the marker itself is never used for I/O.
    resolved.observability.agent_traces.worker_spool_root =
        Some(PathBuf::from("benchmark-owned-trace-spool"));
    let agent_bin = validate_agent_binary(&options.agent_bin)?;
    let program = vec![
        agent_bin
            .to_str()
            .expect("validated UTF-8 agent path")
            .to_string(),
    ];
    let invocation = agent_invocation_with_first_party_program(&resolved, &program)
        .map_err(BenchmarkRunError::LiveConfiguration)?;
    if invocation.supervision != AgentSupervisionKind::FirstParty {
        return Err(BenchmarkRunError::ThirdPartySupervision);
    }
    let mut trace_policy = invocation.trace_policy.clone().ok_or_else(|| {
        BenchmarkRunError::LiveConfiguration(
            "first-party invocation did not provide a trace policy".to_string(),
        )
    })?;
    let redactor = SecretRedactor::from_invocation_env(&invocation.env);
    redactor.ensure_safe_strings(invocation.command.iter(), "agent command")?;

    let manifest = load_benchmark_manifest(&options.benchmark)?;
    let condition = super::condition::resolve_condition(&manifest, options.condition)?;
    if manifest.manifest().capture == CaptureModeV1::Off {
        return Err(BenchmarkRunError::Invalid(
            "live mode requires manifest capture other than `off`".to_string(),
        ));
    }
    ensure_safe_input_snapshots(&redactor, &manifest)?;

    // The manifest defines what a benchmark measures. Retain the production
    // policy's storage and size limits, but make its capture semantics agree
    // with the snapshotted benchmark declaration. Thinking capture is only a
    // valid diagnostic-mode option, so it cannot follow a diagnostic
    // production policy into a less permissive benchmark mode.
    trace_policy.capture = manifest.manifest().capture;
    if trace_policy.capture != CaptureModeV1::Diagnostic {
        trace_policy.capture_thinking = false;
    }
    trace_policy.validate().map_err(|error| {
        BenchmarkRunError::LiveConfiguration(format!(
            "benchmark capture mode produced an invalid trace policy: {error}"
        ))
    })?;
    let runtime = LiveInvocation {
        command: invocation.command,
        env: invocation.env,
        tool_config: super::condition::live_tool_config(
            &manifest,
            condition,
            invocation.tool_config,
        )?,
        runtime_limits: invocation.runtime_limits,
        trace_policy,
        liveness_limits: worker_liveness_limits(&resolved),
    };

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
        let completed = run_live_repetition(
            &manifest, &layout, &runtime, &redactor, repetition, condition,
        )?;
        summaries.push(completed.summary);
        if first_agent_failure.is_none() {
            first_agent_failure = completed.agent_failure;
        }
    }

    let aggregate = aggregate_run_summaries(summaries)?;
    let aggregate = redactor.redacted(&aggregate, "benchmark aggregate")?;
    write_json(&layout.aggregate_json, &aggregate, "benchmark aggregate")?;
    let markdown = render_aggregate_markdown(&aggregate);
    redactor.ensure_safe_bytes(markdown.as_bytes(), "benchmark aggregate Markdown")?;
    write_bytes(
        &layout.aggregate_markdown,
        markdown.as_bytes(),
        "write benchmark aggregate Markdown",
    )?;
    match first_agent_failure {
        Some(error) => Err(error),
        None => Ok(aggregate),
    }
}

fn select_live_worker_pool(
    resolved: &mut temper_config::Resolved,
    requested: Option<&str>,
) -> Result<(), BenchmarkRunError> {
    match requested {
        Some(name) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(BenchmarkRunError::LiveConfiguration(
                    "`--pool` requires a non-empty worker pool name".to_string(),
                ));
            }
            if !resolved.worker.pools.iter().any(|pool| pool.name == name) {
                return Err(BenchmarkRunError::LiveConfiguration(format!(
                    "unknown worker pool `{name}`"
                )));
            }
            resolved.worker.selected_pool = Some(name.to_string());
        }
        None if !resolved.worker.pools.is_empty() => {
            return Err(BenchmarkRunError::LiveConfiguration(
                "worker pools are configured; select the production agent profile with `--pool <NAME>`"
                    .to_string(),
            ));
        }
        None => {}
    }
    Ok(())
}

fn ensure_safe_input_snapshots(
    redactor: &SecretRedactor,
    manifest: &ResolvedBenchmarkManifest,
) -> Result<(), BenchmarkRunError> {
    redactor.ensure_safe_bytes(manifest.source().as_bytes(), "manifest snapshot")?;
    if let Some(expected_patch) = manifest.expected_patch_path() {
        let bytes = std::fs::read(expected_patch).map_err(|source| BenchmarkRunError::Io {
            operation: "read expected patch",
            path: expected_patch.to_path_buf(),
            source,
        })?;
        redactor.ensure_safe_bytes(&bytes, "expected patch snapshot")?;
    }
    let context = serde_json::to_vec(manifest.workspace_context()).map_err(|source| {
        BenchmarkRunError::Json {
            artifact: "workspace context",
            source,
        }
    })?;
    redactor.ensure_safe_bytes(&context, "workspace context snapshot")
}

struct LiveInvocation {
    command: Vec<String>,
    env: Vec<(String, String)>,
    tool_config: Option<AgentToolConfig>,
    runtime_limits: Option<AgentRuntimeLimitsV1>,
    trace_policy: AgentActivityCapturePolicyV1,
    liveness_limits: WorkerLivenessLimits,
}

fn run_live_repetition(
    manifest: &ResolvedBenchmarkManifest,
    layout: &BenchmarkArtifactLayout,
    runtime: &LiveInvocation,
    redactor: &SecretRedactor,
    repetition: u32,
    condition: Option<BenchmarkConditionV1>,
) -> Result<CompletedRepetition, BenchmarkRunError> {
    let workspace = prepare_benchmark_workspace(manifest, repetition)?;
    let paths = layout.snapshot_inputs(repetition, manifest, &workspace)?;
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: runtime.trace_policy.clone(),
        spool_root: Some(workspace.temporary_root().join("trace-spool")),
    });
    let runner = OutOfProcessRunner::new(runtime.command.clone())
        .with_env(runtime.env.clone())
        .with_tool_config(runtime.tool_config.clone())
        .with_runtime_limits(runtime.runtime_limits)
        .with_liveness_limits(runtime.liveness_limits)
        .with_trace_policy(Some(runtime.trace_policy.clone()))
        .with_shared_trace_collector(collector.clone());
    let context = workspace.context().clone();
    let cwd = workspace.root().to_path_buf();
    let job_id = format!("benchmark-live-repetition-{repetition:03}");
    let outcome =
        temper_worker_io::block_on(async move { runner.run(&job_id, &context, &cwd).await });
    let (output, agent_failure) = match outcome {
        Ok(output) => (Some(output), None),
        Err(error) => (
            None,
            Some(BenchmarkRunError::Agent {
                repetition,
                message: redactor.redact_text(&format!("{:?}: {}", error.class, error.message)),
            }),
        ),
    };

    let summary = finalize_repetition(
        manifest,
        &paths,
        &workspace,
        collector,
        output,
        BenchmarkModeV1::Live,
        repetition,
        condition,
        Some(redactor),
    )?;
    Ok(CompletedRepetition {
        summary,
        agent_failure,
    })
}
