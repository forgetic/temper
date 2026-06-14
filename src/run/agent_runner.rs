// SPDX-License-Identifier: MPL-2.0

//! The unified-mode in-process coding-agent runner.
//!
//! [`InProcessAgentRunner`] implements the orchestrator's
//! [`AgentRunner`](temper_worker::AgentRunner) by calling the agent
//! core ([`run_coding_agent_native_with_hooks`]) directly on the host event loop
//! — no subprocess, no temp files. `WorkspaceContext` flows in as a value and
//! `WorkspaceResult` comes back as the return value; step-progress is reported
//! through the injected [`ProgressSink`] in memory.
//!
//! This is the worker→agent carrier `temper run` uses; the split deployment
//! keeps the subprocess `OutOfProcessRunner`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use skein::runtime::RuntimeHandle;
use temper_agent::{CodingAgentError, ProviderConfig, run_coding_agent_native_with_hooks};
use temper_agent_protocol::{PROTOCOL_VERSION, StepProgress, StepState, WorkspaceContext};
use temper_worker::{AgentRunError, AgentRunner, ProgressSink, WorkspaceResult};

/// Runs coding/triage/review turns in-process on the host loop.
pub struct InProcessAgentRunner {
    handle: RuntimeHandle,
    provider: ProviderConfig,
    max_iterations: usize,
    config_dir: Option<PathBuf>,
    enable_subagents: bool,
}

impl InProcessAgentRunner {
    pub fn new(
        handle: RuntimeHandle,
        provider: ProviderConfig,
        max_iterations: usize,
        config_dir: Option<PathBuf>,
        enable_subagents: bool,
    ) -> Self {
        Self {
            handle,
            provider,
            max_iterations,
            config_dir,
            enable_subagents,
        }
    }
}

impl AgentRunner for InProcessAgentRunner {
    fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        progress: &dyn ProgressSink,
    ) -> impl std::future::Future<Output = Result<WorkspaceResult, AgentRunError>> + Send {
        // The terminal marker takes the next free step index. v1 emits the
        // start/finish boundary markers (the subprocess wrapper's outer markers);
        // per-turn checkpoint markers are a follow-up (turn_hook = None here, so
        // the executor commits the final diff itself).
        let step = AtomicU32::new(1);
        let role = context.work_item.role.clone();
        let correlation = context.correlation_key.clone();

        progress.report(StepProgress {
            correlation_key: correlation.clone(),
            step: step.fetch_add(1, Ordering::SeqCst),
            status: format!("start {role} run"),
            state: StepState::Started,
            pushed_sha: None,
            note: Some(format!("protocol v{PROTOCOL_VERSION} (in-process)")),
        });

        let handle = self.handle.clone();
        let provider = self.provider.clone();
        let max_iterations = self.max_iterations;
        let config_dir = self.config_dir.clone();
        let enable_subagents = self.enable_subagents;
        let context = context.clone();
        let cwd = cwd.to_path_buf();

        async move {
            let result = run_coding_agent_native_with_hooks(
                handle,
                &provider,
                &context,
                &cwd,
                max_iterations,
                config_dir.as_deref(),
                enable_subagents,
                None,
                // turn_hook: per-turn checkpointing in-process is issue #166.
                None,
                // checkpoint_hook: ditto (the model-driven checkpoint tool).
                None,
            )
            .await
            .map_err(classify_coding_agent_error)?;

            progress.report(StepProgress {
                correlation_key: correlation,
                step: step.fetch_add(1, Ordering::SeqCst),
                status: format!("finish {role} run"),
                state: StepState::Done,
                pushed_sha: None,
                note: result.summary.clone(),
            });

            Ok(result)
        }
    }
}

/// Map an agent-core error to the worker's transient/permanent classification.
///
/// Mirrors the subprocess path's split: provider/run/abort/model-unavailable are
/// retryable (transient); a missing product, a parse failure, or an undeclared
/// verdict will recur with the same input (permanent).
fn classify_coding_agent_error(error: CodingAgentError) -> AgentRunError {
    match error {
        CodingAgentError::Provider(_)
        | CodingAgentError::Run(_)
        | CodingAgentError::AgentStopped(_)
        | CodingAgentError::ModelUnavailable { .. } => AgentRunError::transient(error.to_string()),
        CodingAgentError::Parse { .. }
        | CodingAgentError::NoProduct
        | CodingAgentError::UndeclaredVerdict { .. } => AgentRunError::permanent(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_worker_protocol::FailureClass;

    #[test]
    fn run_failure_is_transient() {
        let err = classify_coding_agent_error(CodingAgentError::Run("network".into()));
        assert_eq!(err.class, FailureClass::Transient);
    }

    #[test]
    fn model_unavailable_is_transient() {
        let err = classify_coding_agent_error(CodingAgentError::ModelUnavailable {
            model: "x".into(),
            detail: "suspended".into(),
        });
        assert_eq!(err.class, FailureClass::Transient);
    }

    #[test]
    fn no_product_is_permanent() {
        let err = classify_coding_agent_error(CodingAgentError::NoProduct);
        assert_eq!(err.class, FailureClass::Permanent);
    }

    #[test]
    fn undeclared_verdict_is_permanent() {
        let err = classify_coding_agent_error(CodingAgentError::UndeclaredVerdict {
            emitted: "maybe".into(),
            allowed: vec!["yes".into(), "no".into()],
        });
        assert_eq!(err.class, FailureClass::Permanent);
    }
}
