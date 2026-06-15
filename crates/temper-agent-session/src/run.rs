// SPDX-License-Identifier: MPL-2.0

//! The worker ↔ agent protocol driver: read the context, run the native coding
//! loop in cwd (checkpointing on writable jobs), write the result.

use std::path::Path;
use std::sync::Arc;

use temper_agent::{CodingAgentError, ProviderConfig, run_coding_agent_native_with_hooks};
use temper_agent_protocol::{
    CONTEXT_ENV, PROTOCOL_VERSION, RESULT_ENV, StepProgress, StepState, WorkspaceContext,
    WorkspaceResult,
};

use crate::checkpoint::Checkpointer;
use crate::options::Options;
use crate::progress::emit;

pub(crate) fn run<I>(args: I) -> Result<(), String>
where
    I: Iterator<Item = String>,
{
    let options = Options::parse(args)?;
    // Split off the loop knobs before `from_auth` consumes the credential fields,
    // so both halves of `options` can be used without a partial-move borrow.
    let loop_options = LoopOptions {
        max_iterations: options.max_iterations,
        config_dir: options.config_dir.clone(),
        enable_subagents: options.enable_subagents,
    };

    let result_path = std::env::var(RESULT_ENV)
        .map_err(|_| format!("missing required env var {RESULT_ENV} (result file path)"))?;
    let context = read_context()?;

    // Emit the Started checkpoint before any preamble (auth, cwd) so the worker
    // sees the correlation/start even if credential preflight fails.
    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: 1,
        status: format!("start {} run", context.work_item.role),
        state: StepState::Started,
        pushed_sha: None,
        note: Some(format!("protocol v{PROTOCOL_VERSION}")),
    });

    // Provider/model/credential wiring comes from flags (`--auth`, …) and the
    // worker-injected environment (model ids, base-URL override, the
    // materialized OAuth `auth.json` path). `apply_base_url_override_from_env`
    // honors the config-file `[agent.providers.*] url` the worker forwards.
    let provider = ProviderConfig::from_auth(options.auth, options.codex_model, options.auth_file)
        .map_err(|error| format!("provider preflight: {error}"))?
        .apply_base_url_override_from_env();

    // The checkout is our cwd: the worker runs us there, exactly as the legacy
    // file-protocol coder was run.
    let cwd = std::env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?;

    let (checkpointer, resume_note) = prepare_checkpointer(&cwd, &context);
    let result = drive_coding_loop(
        provider,
        &context,
        &cwd,
        loop_options,
        checkpointer.clone(),
        resume_note,
    )
    .map_err(|error| describe_agent_error(&error))?;

    finalize(&result_path, &context, checkpointer.as_deref(), &result)
}

/// Reads and parses the [`WorkspaceContext`] from the file named by
/// [`CONTEXT_ENV`].
fn read_context() -> Result<WorkspaceContext, String> {
    let context_path = std::env::var(CONTEXT_ENV)
        .map_err(|_| format!("missing required env var {CONTEXT_ENV} (context file path)"))?;
    let context_bytes = std::fs::read(&context_path)
        .map_err(|error| format!("read context file {context_path}: {error}"))?;
    serde_json::from_slice(&context_bytes)
        .map_err(|error| format!("parse context file {context_path}: {error}"))
}

/// Builds the checkpointer for writable jobs, recovers any prior pushed
/// checkpoints, emits the resume marker, and aligns step numbering. Returns the
/// checkpointer (absent for read-only jobs) and the resume note for the model.
fn prepare_checkpointer(
    cwd: &Path,
    context: &WorkspaceContext,
) -> (Option<Arc<Checkpointer>>, Option<String>) {
    // Writable jobs checkpoint: commit + push at turn boundaries, resume from
    // prior checkpoints found on the prepared branch. Read-only jobs never
    // mutate the tree, so they run hook-less.
    let writable = context
        .checkout
        .as_deref()
        .map(|capability| capability == "writable")
        .unwrap_or(true);
    let checkpointer = writable.then(|| Arc::new(Checkpointer::new(cwd, context)));
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
/// [`TurnHook`]: temper_agent_core::TurnHook
/// [`CheckpointHook`]: temper_agent::CheckpointHook
fn drive_coding_loop(
    provider: ProviderConfig,
    context: &WorkspaceContext,
    cwd: &Path,
    options: LoopOptions,
    checkpointer: Option<Arc<Checkpointer>>,
    resume_note: Option<String>,
) -> Result<WorkspaceResult, CodingAgentError> {
    // Clone the values the run consumes so the originals survive for the
    // terminal marker; the closure moves only these clones (and the owned
    // `provider`/`options`), so it satisfies the `'static` bound
    // `block_on_with` requires.
    let run_context = context.clone();
    let run_cwd = cwd.to_path_buf();
    temper_agent_io::block_on_with(move |_cx, handle| async move {
        let turn_hook = checkpointer.as_ref().map(Checkpointer::as_turn_hook);
        let checkpoint_hook = checkpointer.as_ref().map(Checkpointer::as_checkpoint_hook);
        run_coding_agent_native_with_hooks(
            handle,
            &provider,
            &run_context,
            &run_cwd,
            options.max_iterations,
            options.config_dir.as_deref(),
            options.enable_subagents,
            resume_note.as_deref(),
            turn_hook,
            checkpoint_hook,
        )
        .await
    })
}

/// The subset of [`Options`] the coding loop needs, split off so the credential
/// fields can be moved into `ProviderConfig::from_auth` independently.
struct LoopOptions {
    max_iterations: usize,
    config_dir: Option<std::path::PathBuf>,
    enable_subagents: bool,
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
