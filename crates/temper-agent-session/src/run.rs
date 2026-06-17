// SPDX-License-Identifier: MPL-2.0

//! The worker ↔ agent protocol driver: drive the native coding loop in cwd
//! (checkpointing on writable jobs) and write the result.
//!
//! Every input — the [`AgentConfig`], the [`WorkspaceContext`], the cwd, and the
//! result-file path — is supplied by [`crate::entry`], the single env-reading
//! module. Nothing here touches `std::env`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use temper_agent::{CodingAgentError, run_coding_agent_native_with_hooks};
use temper_protocol_agent::{
    PROTOCOL_VERSION, StepProgress, StepState, WorkspaceContext, WorkspaceResult,
};

use crate::checkpoint::Checkpointer;
use crate::config::AgentConfig;
use crate::progress::emit;

/// Drives one protocol run from fully-resolved inputs.
///
/// `config` carries the provider wiring + the host-derived session knobs;
/// `context` is the already-parsed workspace context; `cwd` is the prepared
/// checkout the worker handed us; `result_path` is the file the result is written
/// to. All four are resolved (and the env read) in [`crate::entry`].
pub(crate) fn drive(
    config: AgentConfig,
    context: WorkspaceContext,
    cwd: PathBuf,
    result_path: String,
) -> Result<(), String> {
    // Emit the Started checkpoint before any preamble so the worker sees the
    // correlation/start even if the coding loop fails early.
    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: 1,
        status: format!("start {} run", context.work_item.role),
        state: StepState::Started,
        pushed_sha: None,
        note: Some(format!("protocol v{PROTOCOL_VERSION}")),
    });

    let (checkpointer, resume_note) = prepare_checkpointer(&cwd, &context, &config);
    let result = drive_coding_loop(&config, &context, &cwd, checkpointer.clone(), resume_note)
        .map_err(|error| describe_agent_error(&error))?;

    finalize(&result_path, &context, checkpointer.as_deref(), &result)
}

/// Builds the checkpointer for writable jobs, recovers any prior pushed
/// checkpoints, emits the resume marker, and aligns step numbering. Returns the
/// checkpointer (absent for read-only jobs) and the resume note for the model.
fn prepare_checkpointer(
    cwd: &Path,
    context: &WorkspaceContext,
    config: &AgentConfig,
) -> (Option<Arc<Checkpointer>>, Option<String>) {
    // Writable jobs checkpoint: commit + push at turn boundaries, resume from
    // prior checkpoints found on the prepared branch. Read-only jobs never
    // mutate the tree, so they run hook-less.
    let writable = context
        .checkout
        .as_deref()
        .map(|capability| capability == "writable")
        .unwrap_or(true);
    let checkpointer = writable.then(|| {
        Arc::new(Checkpointer::new(
            cwd,
            context,
            config.deadline,
            config.checkpoint_interval,
        ))
    });
    let resume = checkpointer
        .as_deref()
        .and_then(Checkpointer::detect_resume);
    if let Some(resume) = &resume {
        emit(&StepProgress {
            correlation_key: context.correlation_key.clone(),
            step: resume.last_step + 1,
            status: format!(
                "resume {} run from pushed checkpoints",
                context.work_item.role
            ),
            state: StepState::Started,
            pushed_sha: Some(resume.head_sha.clone()),
            note: Some(format!(
                "{} checkpoint commit(s) on the branch",
                resume.commits
            )),
        });
    }
    if let Some(checkpointer) = &checkpointer {
        checkpointer.start_after(resume.as_ref().map(|resume| resume.last_step + 1));
    }
    let resume_note = resume.as_ref().map(|resume| {
        format!(
            "A previous run of this task was interrupted after pushing {} checkpoint              commit(s); the working tree already reflects them:\n{}\nContinue from              that state — do not redo work those commits already contain.",
            resume.commits, resume.log
        )
    });
    (checkpointer, resume_note)
}

/// Runs the native coding loop on the async runtime, wiring the checkpointer in
/// as both the mechanical backstop ([`TurnHook`]) and the model-driven
/// `checkpoint` tool ([`CheckpointHook`]).
///
/// Takes the session's per-subsystem [`AgentConfig`] (issue #199): the provider,
/// iteration cap, config dir, and sub-agent toggle all come from it.
///
/// [`TurnHook`]: temper_agent_core::TurnHook
/// [`CheckpointHook`]: temper_agent::CheckpointHook
fn drive_coding_loop(
    config: &AgentConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    checkpointer: Option<Arc<Checkpointer>>,
    resume_note: Option<String>,
) -> Result<WorkspaceResult, CodingAgentError> {
    // Clone the values the run consumes so the originals survive for the
    // terminal marker; the closure moves only these clones (and the owned
    // config knobs), so it satisfies the `'static` bound `block_on_with`
    // requires.
    let run_context = context.clone();
    let run_cwd = cwd.to_path_buf();
    let provider = config.provider.clone();
    let max_iterations = config.max_iterations;
    let config_dir = config.config_dir.clone();
    let enable_subagents = config.enable_subagents;
    temper_agent_io::block_on_with(move |_cx, handle| async move {
        let turn_hook = checkpointer.as_ref().map(Checkpointer::as_turn_hook);
        let checkpoint_hook = checkpointer.as_ref().map(Checkpointer::as_checkpoint_hook);
        run_coding_agent_native_with_hooks(
            handle,
            &provider,
            &run_context,
            &run_cwd,
            max_iterations,
            config_dir.as_deref(),
            enable_subagents,
            resume_note.as_deref(),
            turn_hook,
            checkpoint_hook,
        )
        .await
        // The session path doesn't surface token totals; drop them here.
        .map(|(result, _totals)| result)
    })
}

/// Emits the terminal Done marker (taking the next free step index after any
/// pushed checkpoints) and writes the result file.
fn finalize(
    result_path: &str,
    context: &WorkspaceContext,
    checkpointer: Option<&Checkpointer>,
    result: &WorkspaceResult,
) -> Result<(), String> {
    let final_step = checkpointer.map(Checkpointer::next_step).unwrap_or(2);
    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: final_step,
        status: format!("finish {} run", context.work_item.role),
        state: StepState::Done,
        pushed_sha: None,
        note: result.summary.clone(),
    });
    write_result(result_path, result)
}

fn write_result(result_path: &str, result: &WorkspaceResult) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(result).map_err(|error| format!("serialize result: {error}"))?;
    std::fs::write(result_path, bytes)
        .map_err(|error| format!("write result file {result_path}: {error}"))
}

/// Renders a coding-agent error for stderr. The worker re-derives the
/// transient/permanent class from the process exit (non-zero ⇒ transient) plus
/// a missing result file (⇒ permanent); the message here is for humans.
///
/// A model-unavailability rejection (a suspended model alias, or a tier that
/// does not grant the configured model) is flagged with an explicit
/// `model-unavailable:` prefix so it stands out in logs as a configuration
/// problem the `Display` text already explains how to fix — not a transient
/// network blip to be retried blindly.
fn describe_agent_error(error: &CodingAgentError) -> String {
    match error {
        CodingAgentError::ModelUnavailable { .. } => {
            format!("model-unavailable: {error}")
        }
        _ => format!("coding agent failed: {error}"),
    }
}
