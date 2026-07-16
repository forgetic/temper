//! Per-attempt orchestration around the joined process supervisor.

use std::path::{Path, PathBuf};

use crate::trace::ActivityEndpoint;

use super::*;

struct ForgeHostTask {
    future: AgentForgeContextFuture,
    response: std::sync::mpsc::SyncSender<ForgeContextResponse>,
}

/// Every blocking or threaded resource owned by one attempt. Drop is the
/// cancellation boundary used when the worker watchdog or component owner
/// drops the run future.
struct RunResources {
    job_id: String,
    fence: AttemptFence,
    accepted_submit: AcceptedSubmitProofStore,
    process: Option<ManagedAgentProcess>,
    lifecycle: Option<lifecycle::LifecycleEndpoint>,
    activity: Option<ActivityEndpoint>,
    submit: Option<LocalServer>,
    forge: Option<LocalServer>,
    limits: WorkerLivenessLimits,
    finished: bool,
}

impl RunResources {
    fn process_mut(&mut self) -> &mut ManagedAgentProcess {
        self.process
            .as_mut()
            .expect("run resources always own a process until quiescence")
    }

    fn finish(mut self, result: SupervisorResult) -> SupervisorResult {
        if let Some(process) = self.process.as_mut() {
            process.join_completed();
        }
        self.process.take();
        self.stop_endpoints();
        self.finished = true;
        emit_quiesced(&self.job_id, &result.quiesced, false);
        result
    }

    fn stop_endpoints(&mut self) {
        if let Some(server) = self.submit.take() {
            server.stop();
        }
        if let Some(server) = self.forge.take() {
            server.stop();
        }
        if let Some(endpoint) = self.activity.take() {
            endpoint.stop();
        }
        if let Some(endpoint) = self.lifecycle.take() {
            endpoint.stop();
        }
    }
}

impl Drop for RunResources {
    fn drop(&mut self) {
        if self.finished || self.process.is_none() {
            return;
        }

        // Fence first: no late side-channel completion or result file can
        // become authoritative after cancellation starts.
        self.fence.close();
        self.accepted_submit.clear();
        let first_party_connected = self
            .lifecycle
            .as_ref()
            .is_some_and(lifecycle::LifecycleEndpoint::connected);
        if first_party_connected {
            let _ = self
                .lifecycle
                .as_ref()
                .expect("connected lifecycle endpoint exists")
                .request_cancel("worker cancelled agent attempt");
        }

        let result = self
            .process
            .as_mut()
            .expect("process exists")
            .cancel_and_join(
                first_party_connected,
                self.limits.graceful_cancellation_grace,
                self.limits.forced_termination_grace,
            );
        self.process.take();
        self.stop_endpoints();
        // Clear again after joining accepted handlers. A submit gate that was
        // already running when the fence closed cannot leave proof behind.
        self.accepted_submit.clear();
        emit_quiesced(&self.job_id, &result.quiesced, true);
        self.finished = true;
    }
}

fn emit_quiesced(job_id: &str, outcome: &JobQuiesced, cancelled: bool) {
    if cancelled {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "worker.job.quiesced",
            job_id,
            cancellation = ?outcome.cancellation,
            descendant_cleanup = ?outcome.descendants,
            containment = ?outcome.containment,
            "agent run cancelled and all owned process resources joined"
        );
    } else {
        tracing::debug!(
            target: "temper::worker",
            service = "worker",
            event = "worker.job.quiesced",
            job_id,
            cancellation = ?outcome.cancellation,
            descendant_cleanup = ?outcome.descendants,
            containment = ?outcome.containment,
            "agent run completed and all owned process resources joined"
        );
    }
}

impl OutOfProcessRunner {
    pub(super) async fn run_agent(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
        trace: Option<&TraceRun>,
        progress: crate::JobProgressReporter,
    ) -> Result<AgentRunOutput, AgentRunError> {
        let Some((program, args)) = self.command.split_first() else {
            return Err(AgentRunError::permanent("agent command is empty"));
        };

        let temp = tempfile::tempdir()
            .map_err(|error| AgentRunError::transient(format!("create agent temp dir: {error}")))?;
        let context_path = temp.path().join("context.json");
        let result_path = temp.path().join("result.json");
        let context_bytes = serde_json::to_vec_pretty(context).map_err(|error| {
            AgentRunError::transient(format!("serialize agent context: {error}"))
        })?;
        std::fs::write(&context_path, context_bytes).map_err(|error| {
            AgentRunError::transient(format!("write agent context file: {error}"))
        })?;

        let tool_config_path = self.write_tool_config(temp.path(), context)?;
        let runtime_limits_path = runtime_limits::write(temp.path(), self.runtime_limits)?;
        let trace_policy_path = self.write_trace_policy(temp.path(), job_id);

        let submit_listener = optional_listener(
            submit_for_pr_available(context),
            "submit_for_pr side channel",
        )?;
        let forge_listener =
            optional_listener(self.forge_context.is_some(), "Forge context side channel")?;

        // Runtime limits are the compatibility signal for a known first-party
        // child. Third-party commands receive neither flag.
        let lifecycle_endpoint = if self.runtime_limits.is_some() {
            Some(
                lifecycle::LifecycleEndpoint::bind(progress).map_err(|error| {
                    AgentRunError::transient(format!("bind agent lifecycle endpoint: {error}"))
                })?,
            )
        } else {
            None
        };
        let activity_endpoint = if self.trace_policy.is_some() {
            trace.and_then(|trace| match trace.bind_endpoint() {
                Ok(endpoint) => Some(endpoint),
                Err(error) => {
                    tracing::warn!(
                        target: "temper::worker",
                        service = "worker",
                        event = "agent.activity.endpoint_failed",
                        run_id = trace.run_id(),
                        job_id,
                        %error,
                        "worker could not bind the child activity endpoint; continuing without child activity"
                    );
                    None
                }
            })
        } else {
            None
        };

        let fence = AttemptFence::open();
        let accepted_submit = AcceptedSubmitProofStore::new();
        let (forge_requests, mut forge_request_rx) = temper_worker_io::channel();
        let command = self.child_command(
            program,
            args,
            cwd,
            &context_path,
            &result_path,
            tool_config_path.as_deref(),
            runtime_limits_path.as_deref(),
            trace_policy_path.as_deref(),
            lifecycle_endpoint
                .as_ref()
                .map(|endpoint| endpoint.address()),
            activity_endpoint.as_ref().map(ActivityEndpoint::address),
            submit_listener
                .as_ref()
                .map(|(_, address)| address.as_str()),
            forge_listener.as_ref().map(|(_, address)| address.as_str()),
        );

        let submit_server = submit_listener.map(|(listener, address)| {
            start_submit_server(
                listener,
                address,
                Arc::clone(&self.submit_for_pr),
                accepted_submit.clone(),
                context.clone(),
                cwd.to_path_buf(),
                fence.clone(),
            )
        });
        let forge_server = forge_listener.map(|(listener, address)| {
            start_forge_server(listener, address, forge_requests, fence.clone())
        });
        let identity = DiagnosticIdentity::from_context(job_id, context);
        let process = ManagedAgentProcess::spawn(command, identity, self.diagnostic_dispatch())?;
        let mut resources = RunResources {
            job_id: job_id.to_string(),
            fence: fence.clone(),
            accepted_submit: accepted_submit.clone(),
            process: Some(process),
            lifecycle: lifecycle_endpoint,
            activity: activity_endpoint,
            submit: submit_server,
            forge: forge_server,
            limits: self.liveness_limits,
            finished: false,
        };

        let forge_context = self.forge_context.clone();
        let bound_job_id = job_id.to_string();
        let mut pending_forge: Option<ForgeHostTask> = None;
        let mut forge_closed = false;
        let supervisor_result = loop {
            enum Next {
                Child(SupervisorResult),
                ForgeRequest(Option<ForgeSideChannelRequest>),
                ForgeCompleted(
                    Result<
                        temper_protocol_agent::ForgeContextResult,
                        temper_protocol_agent::ForgeContextErrorCode,
                    >,
                ),
            }

            let next = std::future::poll_fn(|task_cx| {
                if let Poll::Ready(outcome) = resources.process_mut().poll_outcome(task_cx) {
                    return Poll::Ready(Next::Child(outcome));
                }
                if let Some(task) = pending_forge.as_mut() {
                    if let Poll::Ready(outcome) = task.future.as_mut().poll(task_cx) {
                        return Poll::Ready(Next::ForgeCompleted(outcome));
                    }
                } else if !forge_closed {
                    let mut receive = Box::pin(forge_request_rx.recv());
                    if let Poll::Ready(request) = receive.as_mut().poll(task_cx) {
                        return Poll::Ready(Next::ForgeRequest(request));
                    }
                }
                Poll::Pending
            })
            .await;

            match next {
                Next::Child(outcome) => {
                    if let Some(task) = pending_forge.take() {
                        let _ = task.response.send(forge_unavailable());
                    }
                    break outcome;
                }
                Next::ForgeRequest(Some(request)) => match &forge_context {
                    Some(host) if fence.is_open() => {
                        pending_forge = Some(ForgeHostTask {
                            future: host(bound_job_id.clone(), request.operation),
                            response: request.response,
                        });
                    }
                    Some(_) => {
                        let _ = request.response.send(forge_unavailable());
                    }
                    None => {
                        let _ = request.response.send(ForgeContextResponse::error(
                            temper_protocol_agent::ForgeContextErrorCode::NotAuthorized,
                        ));
                    }
                },
                Next::ForgeRequest(None) => forge_closed = true,
                Next::ForgeCompleted(outcome) => {
                    let task = pending_forge
                        .take()
                        .expect("completed Forge task remains attempt-bound");
                    let response = if !fence.is_open() {
                        forge_unavailable()
                    } else {
                        match outcome {
                            Ok(result) => ForgeContextResponse::success(result),
                            Err(code) => ForgeContextResponse::error(code),
                        }
                    };
                    let _ = task.response.send(response);
                }
            }
        };

        let supervisor_result = resources.finish(supervisor_result);
        let ChildOutcome {
            status_code,
            stderr_tail,
        } = supervisor_result.outcome?;
        match status_code {
            Some(0) => {}
            Some(code) => {
                return Err(AgentRunError::transient(format!(
                    "agent command exited with status {code}; stderr tail: {stderr_tail}"
                )));
            }
            None => {
                return Err(AgentRunError::transient(format!(
                    "agent command terminated without an exit code; stderr tail: {stderr_tail}"
                )));
            }
        }

        if !fence.is_open() {
            return Err(AgentRunError::transient(
                "agent attempt was cancelled before result acceptance",
            ));
        }
        let result_bytes = std::fs::read(&result_path).map_err(|error| {
            AgentRunError::permanent(format!("agent did not write a valid result file: {error}"))
        })?;
        if !fence.is_open() {
            return Err(AgentRunError::transient(
                "agent attempt was cancelled while reading its result",
            ));
        }
        let result = serde_json::from_slice::<WorkspaceResult>(&result_bytes).map_err(|error| {
            AgentRunError::permanent(format!("agent result file is not valid JSON: {error}"))
        })?;
        Ok(AgentRunOutput {
            result,
            accepted_submit: fence.is_open().then(|| accepted_submit.latest()).flatten(),
        })
    }

    fn write_tool_config(
        &self,
        directory: &Path,
        context: &WorkspaceContext,
    ) -> Result<Option<PathBuf>, AgentRunError> {
        let Some(tool_config) = self
            .tool_config
            .as_ref()
            .filter(|config| config.enabled_for_role(&context.work_item.role))
        else {
            return Ok(None);
        };
        let path = directory.join("tool-config.json");
        let bytes = serde_json::to_vec_pretty(tool_config).map_err(|error| {
            AgentRunError::transient(format!("serialize agent tool config: {error}"))
        })?;
        std::fs::write(&path, bytes).map_err(|error| {
            AgentRunError::transient(format!("write agent tool config file: {error}"))
        })?;
        Ok(Some(path))
    }

    fn write_trace_policy(&self, directory: &Path, job_id: &str) -> Option<PathBuf> {
        let policy = self.trace_policy.as_ref()?;
        let path = directory.join("trace-policy.json");
        let bytes = match serde_json::to_vec_pretty(policy) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.policy_serialize_failed",
                    job_id,
                    %error,
                    "worker could not serialize agent trace policy; continuing without child activity"
                );
                return None;
            }
        };
        if let Err(error) = std::fs::write(&path, bytes) {
            tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.policy_write_failed",
                job_id,
                path = %path.display(),
                %error,
                "worker could not write agent trace policy; continuing without child activity"
            );
            None
        } else {
            Some(path)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn child_command(
        &self,
        program: &str,
        args: &[String],
        cwd: &Path,
        context_path: &Path,
        result_path: &Path,
        tool_config_path: Option<&Path>,
        runtime_limits_path: Option<&Path>,
        trace_policy_path: Option<&Path>,
        lifecycle_address: Option<&str>,
        activity_address: Option<&str>,
        submit_address: Option<&str>,
        forge_address: Option<&str>,
    ) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .arg("--context")
            .arg(context_path)
            .arg("--result")
            .arg(result_path)
            .arg("--workspace")
            .arg(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(path) = tool_config_path {
            command.arg(TOOL_CONFIG_FLAG).arg(path);
        }
        if let Some(path) = runtime_limits_path {
            command.arg(RUNTIME_LIMITS_FLAG).arg(path);
        }
        if let Some(path) = trace_policy_path {
            command.arg(TRACE_POLICY_FLAG).arg(path);
        }
        if let Some(address) = lifecycle_address {
            command.arg(AGENT_LIFECYCLE_ADDRESS_FLAG).arg(address);
        }
        if let Some(address) = activity_address {
            command.arg(ACTIVITY_ADDRESS_FLAG).arg(address);
        }
        if let Some(address) = submit_address {
            command.arg(SUBMIT_FOR_PR_ADDRESS_FLAG).arg(address);
        }
        if let Some(address) = forge_address {
            command.arg(FORGE_CONTEXT_ADDRESS_FLAG).arg(address);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}

fn optional_listener(
    enabled: bool,
    label: &str,
) -> Result<Option<(TcpListener, String)>, AgentRunError> {
    if !enabled {
        return Ok(None);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| AgentRunError::transient(format!("bind {label}: {error}")))?;
    let address = listener
        .local_addr()
        .map_err(|error| AgentRunError::transient(format!("read {label} address: {error}")))?;
    Ok(Some((listener, address.to_string())))
}

fn forge_unavailable() -> ForgeContextResponse {
    ForgeContextResponse::error(temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable)
}
