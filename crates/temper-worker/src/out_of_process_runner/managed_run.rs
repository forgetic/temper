//! Per-attempt orchestration around the joined process supervisor.

use std::path::Path;
use std::time::Duration;

use crate::executor::{JobCancellationRequest, ResourceJoinReport, ResourceJoinStatus};
use crate::managed_effect::JoinedBlocking;
use crate::trace::ActivityEndpoint;

use super::*;

struct ForgeHostTask {
    future: AgentForgeContextFuture,
    response: std::sync::mpsc::SyncSender<ForgeContextResponse>,
}

struct SubmitHostTask {
    future: SubmitForPrFuture,
    response: std::sync::mpsc::SyncSender<SubmitForPrResponse>,
}

const LIFECYCLE_CONNECT_GRACE: Duration = Duration::from_millis(100);

struct AttemptCleanupObserver(JobCancellation);

impl temper_process_containment::CleanupObserver for AttemptCleanupObserver {
    fn observe(&self, snapshot: &temper_process_containment::CleanupSnapshot) {
        if matches!(
            snapshot,
            temper_process_containment::CleanupSnapshot::Blocked { .. }
        ) {
            self.0.observe_cleanup(snapshot.clone());
        }
    }
}

/// Every blocking or threaded resource owned by one attempt. The explicit
/// cancellation path drives this owner to `finish`; Drop is only the abrupt
/// component-loss hard-kill fallback.
struct RunResources {
    job_id: String,
    fence: AttemptFence,
    accepted_submit: AcceptedSubmitProofStore,
    process: Option<ManagedAgentProcess>,
    lifecycle: Option<lifecycle::LifecycleEndpoint>,
    activity: Option<ActivityEndpoint>,
    trace: Option<TraceRun>,
    submit: Option<LocalServer>,
    forge: Option<LocalServer>,
    finished: bool,
}

impl RunResources {
    fn process_mut(&mut self) -> &mut ManagedAgentProcess {
        self.process
            .as_mut()
            .expect("run resources always own a process until quiescence")
    }

    fn finish(
        mut self,
        mut result: SupervisorResult,
        cancelled: bool,
        lifecycle_cancellation: ResourceJoinStatus,
    ) -> SupervisorResult {
        result.quiesced.cleanup.resources.process_supervisor = if self
            .process
            .as_mut()
            .is_some_and(ManagedAgentProcess::join_completed)
        {
            ResourceJoinStatus::Joined
        } else {
            ResourceJoinStatus::Failed("agent supervisor thread panicked".to_string())
        };
        self.process.take();
        self.stop_endpoints(&mut result.quiesced.cleanup.resources);
        result.quiesced.cleanup.resources.lifecycle_cancellation = lifecycle_cancellation;
        if cancelled {
            self.finish_cancelled_activity();
            // Clear again after joining accepted handlers. A submit gate that
            // was already running when the fence closed cannot leave proof.
            self.accepted_submit.clear();
        }
        self.finished = true;
        emit_quiesced(&self.job_id, &result.quiesced, cancelled);
        result
    }

    fn finish_cancelled_activity(&self) {
        let Some(trace) = self.trace.as_ref() else {
            return;
        };
        match trace.finish_cancelled() {
            Ok(_) | Err(crate::trace::TraceError::AlreadyTerminal) => {}
            Err(error) => tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.terminal_failed",
                run_id = trace.run_id(),
                job_id = self.job_id,
                %error,
                "worker could not persist synthetic cancelled terminal activity"
            ),
        }
    }

    fn stop_endpoints(&mut self, report: &mut ResourceJoinReport) {
        if let Some(server) = self.submit.take() {
            report.submit_endpoint = join_status(server.stop(), "submit endpoint");
        }
        if let Some(server) = self.forge.take() {
            report.forge_endpoint = join_status(server.stop(), "Forge endpoint");
        }
        if let Some(endpoint) = self.activity.take() {
            report.activity_endpoint = join_status(endpoint.stop(), "activity endpoint");
        }
        if let Some(endpoint) = self.lifecycle.take() {
            report.lifecycle_endpoint = join_status(endpoint.stop(), "lifecycle endpoint");
        }
    }
}

impl Drop for RunResources {
    fn drop(&mut self) {
        if self.finished || self.process.is_none() {
            return;
        }

        // Abrupt owner loss is a last-resort safety path. Watchdog
        // cancellation stays in the async run loop below and never waits for
        // the process supervisor from Drop.
        self.fence.close();
        self.accepted_submit.clear();
        self.process.take();
        let mut ignored = ResourceJoinReport::no_process();
        self.stop_endpoints(&mut ignored);
        self.finish_cancelled_activity();
        self.accepted_submit.clear();
        self.finished = true;
    }
}

fn join_status(joined: bool, resource: &str) -> ResourceJoinStatus {
    if joined {
        ResourceJoinStatus::Joined
    } else {
        ResourceJoinStatus::Failed(format!("{resource} thread panicked"))
    }
}

fn emit_quiesced(job_id: &str, outcome: &JobQuiesced, cancelled: bool) {
    let cleanup = &outcome.cleanup;
    let report = &cleanup.containment;
    let recovered = !report.observed_survivors().is_empty()
        || report.omitted_survivors() > 0
        || !matches!(
            report.disposition(),
            temper_process_containment::CleanupDisposition::AlreadyEmpty
        );
    if cancelled || recovered || !cleanup.proves_quiescence() {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "worker.job.quiesced",
            job_id,
            cancellation = ?cleanup.cancellation,
            backend = ?report.backend(),
            root = report.root().value(),
            disposition = ?report.disposition(),
            resources = ?cleanup.resources,
            "agent run cleanup recovered descendants or followed cancellation"
        );
    } else {
        tracing::debug!(
            target: "temper::worker",
            service = "worker",
            event = "worker.job.quiesced",
            job_id,
            cancellation = ?cleanup.cancellation,
            backend = ?report.backend(),
            root = report.root().value(),
            disposition = ?report.disposition(),
            resources = ?cleanup.resources,
            "agent run completed with recursive emptiness and resource joins proven"
        );
    }
}

impl OutOfProcessRunner {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent(
        &self,
        job_id: &str,
        attempt_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
        trace: Option<&TraceRun>,
        progress: crate::JobProgressReporter,
        fence: AttemptFence,
        cancellation: JobCancellation,
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
        let mut lifecycle_endpoint = if self.runtime_limits.is_some() {
            Some(
                lifecycle::LifecycleEndpoint::bind(progress).map_err(|error| {
                    AgentRunError::transient(format!("bind agent lifecycle endpoint: {error}"))
                })?,
            )
        } else {
            None
        };
        let mut activity_endpoint = if self.trace_policy.is_some() {
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

        let accepted_submit = AcceptedSubmitProofStore::new();
        let (submit_requests, mut submit_request_rx) = temper_worker_io::channel();
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

        let containment_factory =
            (self.containment_factory)(job_id, attempt_id).map_err(|error| {
                AgentRunError::transient(format!("create agent containment factory: {error}"))
            })?;
        let containment_factory = containment_factory
            .with_observer(Arc::new(AttemptCleanupObserver(cancellation.clone())));
        let containment_spec = temper_process_containment::ContainmentSpec::new(
            temper_process_containment::ContainmentIdentity::new(format!(
                "job-{job_id}-attempt-{attempt_id}"
            ))
            .map_err(|error| {
                AgentRunError::transient(format!("identify agent containment: {error}"))
            })?,
            ContainmentScope::Job,
        )
        .with_timing(
            self.liveness_limits.forced_termination_grace,
            Duration::from_millis(100),
        );
        // Preparation establishes the outer ownership boundary before any
        // untrusted agent instruction can execute. The cgroup backend passes
        // its inherited scope descriptor to first-party children so their tool
        // containments are nested below this final safety net.
        let prepared = containment_factory
            .prepare(containment_spec)
            .map_err(|error| {
                AgentRunError::transient(format!("prepare agent containment: {error}"))
            })?;
        let mut submit_server = submit_listener
            .map(|(listener, address)| {
                start_submit_server(listener, address, submit_requests, fence.clone())
            })
            .transpose()
            .map_err(|error| {
                AgentRunError::transient(format!("start submit side-channel owner: {error}"))
            })?;
        let mut forge_server = forge_listener
            .map(|(listener, address)| {
                start_forge_server(listener, address, forge_requests, fence.clone())
            })
            .transpose()
            .map_err(|error| {
                AgentRunError::transient(format!("start Forge side-channel owner: {error}"))
            })?;
        let identity = DiagnosticIdentity::from_context(job_id, context);
        let process = match ManagedAgentProcess::spawn(
            prepared,
            command,
            identity,
            self.diagnostic_dispatch(),
        ) {
            Ok(process) => process,
            Err(mut failure) => {
                if let Some(server) = submit_server.take() {
                    failure.cleanup.resources.submit_endpoint =
                        join_status(server.stop(), "submit endpoint");
                }
                if let Some(server) = forge_server.take() {
                    failure.cleanup.resources.forge_endpoint =
                        join_status(server.stop(), "Forge endpoint");
                }
                if let Some(endpoint) = activity_endpoint.take() {
                    failure.cleanup.resources.activity_endpoint =
                        join_status(endpoint.stop(), "activity endpoint");
                }
                if let Some(endpoint) = lifecycle_endpoint.take() {
                    failure.cleanup.resources.lifecycle_endpoint =
                        join_status(endpoint.stop(), "lifecycle endpoint");
                }
                if !failure.cleanup.proves_quiescence() {
                    cancellation.observe_cleanup(
                        temper_process_containment::CleanupSnapshot::Blocked {
                            trigger: failure.cleanup.containment.trigger(),
                            phase: temper_process_containment::CleanupPhase::Reap,
                            message: "agent setup resource join failed".to_string(),
                            survivors: failure.cleanup.containment.observed_survivors().to_vec(),
                            omitted_survivors: failure.cleanup.containment.omitted_survivors(),
                        },
                    );
                }
                let _ = cancellation.record_cleanup(failure.cleanup);
                return Err(failure.error);
            }
        };
        let _cancellation_owner = cancellation.register_async_owner();
        let mut resources = RunResources {
            job_id: job_id.to_string(),
            fence: fence.clone(),
            accepted_submit: accepted_submit.clone(),
            process: Some(process),
            lifecycle: lifecycle_endpoint,
            activity: activity_endpoint,
            trace: trace.cloned(),
            submit: submit_server,
            forge: forge_server,
            finished: false,
        };

        let forge_context = self.forge_context.clone();
        let operation_timeout =
            Duration::from_secs(self.runtime_limits.unwrap_or_default().tool_timeout_secs);
        let submit_for_pr = Arc::clone(&self.submit_for_pr);
        let submit_context = context.clone();
        let submit_cwd = cwd.to_path_buf();
        let bound_job_id = job_id.to_string();
        let mut pending_forge: Option<ForgeHostTask> = None;
        let mut pending_submit: Option<SubmitHostTask> = None;
        let mut forge_closed = false;
        let mut submit_closed = false;
        let mut observed_cancellation = None;
        let mut _lifecycle_cancel = None;
        let supervisor_result = loop {
            enum Next {
                Cancellation(JobCancellationRequest),
                Child(SupervisorResult),
                ForgeRequest(Option<ForgeSideChannelRequest>),
                ForgeCompleted(
                    Result<
                        temper_protocol_agent::ForgeContextResult,
                        temper_protocol_agent::ForgeContextErrorCode,
                    >,
                ),
                SubmitRequest(Option<SubmitSideChannelRequest>),
                SubmitCompleted(SubmitForPrResponse),
            }

            let next = std::future::poll_fn(|task_cx| {
                // Cancellation wins a same-poll race with natural child exit:
                // once WorkerMachine closes the attempt fence it must receive
                // one cancellation report, never a normal result.
                if let Poll::Ready(request) =
                    cancellation.poll_request(observed_cancellation, task_cx)
                {
                    return Poll::Ready(Next::Cancellation(request));
                }
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
                if let Some(task) = pending_submit.as_mut() {
                    if let Poll::Ready(response) = task.future.as_mut().poll(task_cx) {
                        return Poll::Ready(Next::SubmitCompleted(response));
                    }
                } else if !submit_closed {
                    let mut receive = Box::pin(submit_request_rx.recv());
                    if let Poll::Ready(request) = receive.as_mut().poll(task_cx) {
                        return Poll::Ready(Next::SubmitRequest(request));
                    }
                }
                Poll::Pending
            })
            .await;

            match next {
                Next::Cancellation(request) => {
                    if observed_cancellation.is_none() {
                        // Fence before touching the child. No late host response
                        // or result file can become authoritative after this
                        // point.
                        fence.close();
                        accepted_submit.clear();
                        if let Some(task) = pending_forge.take() {
                            let _ = task.response.send(forge_unavailable());
                        }
                        if let Some(task) = pending_submit.take() {
                            let _ = task
                                .response
                                .send(SubmitForPrResponse::rejected("agent attempt was cancelled"));
                        }
                    }
                    observed_cancellation = Some(request);
                    match request {
                        JobCancellationRequest::Graceful => {
                            if let Some(endpoint) = resources.lifecycle.as_ref() {
                                let handle = endpoint.cancellation_handle();
                                _lifecycle_cancel = Some(JoinedBlocking::spawn(
                                    "agent-lifecycle-cancel",
                                    move || {
                                        handle.request_cancel(
                                            "worker cancelled agent attempt",
                                            LIFECYCLE_CONNECT_GRACE,
                                        )
                                    },
                                ));
                            }
                            let _ = resources.process_mut().request_cancel();
                        }
                        JobCancellationRequest::ForcedTermination => {
                            let _ = resources.process_mut().force_terminate();
                        }
                        JobCancellationRequest::HardKill => {
                            let _ = resources.process_mut().hard_kill();
                        }
                    }
                }
                Next::Child(mut outcome) => {
                    if let Some(task) = pending_forge.take() {
                        let _ = task.response.send(forge_unavailable());
                    }
                    if let Some(task) = pending_submit.take() {
                        let _ = task.response.send(SubmitForPrResponse::rejected(
                            "agent attempt ended before submit_for_pr completed",
                        ));
                    }
                    // A natural exit may race command receipt after the attempt
                    // fence has closed. Project that as the cooperative outcome
                    // that actually won, rather than losing the cancellation
                    // completion entirely.
                    if outcome.quiesced.cleanup.cancellation.is_none()
                        && cancellation.is_cancelled()
                    {
                        outcome.quiesced.cleanup.cancellation = Some(CancellationOutcome::Graceful);
                    }
                    break outcome;
                }
                Next::ForgeRequest(Some(request)) => match &forge_context {
                    Some(host) if fence.is_open() => {
                        pending_forge = Some(ForgeHostTask {
                            future: bounded_forge_future(
                                host(bound_job_id.clone(), request.operation),
                                operation_timeout,
                            ),
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
                Next::SubmitRequest(Some(request)) if fence.is_open() => {
                    pending_submit = Some(SubmitHostTask {
                        future: bounded_submit_future(
                            submit_for_pr(
                                request.request,
                                submit_context.clone(),
                                submit_cwd.clone(),
                                cancellation.clone(),
                            ),
                            operation_timeout,
                        ),
                        response: request.response,
                    });
                }
                Next::SubmitRequest(Some(request)) => {
                    let _ = request.response.send(SubmitForPrResponse::rejected(
                        "agent attempt is no longer available",
                    ));
                }
                Next::SubmitRequest(None) => submit_closed = true,
                Next::SubmitCompleted(response) => {
                    let task = pending_submit
                        .take()
                        .expect("completed submit task remains attempt-bound");
                    let response = if fence.is_open() {
                        accepted_submit
                            .record_response_controlled(response, context, cwd, &cancellation)
                            .await
                    } else {
                        accepted_submit.clear();
                        SubmitForPrResponse::rejected("agent attempt is no longer available")
                    };
                    let _ = task.response.send(response);
                }
            }
        };

        let cancelled = cancellation.is_cancelled()
            || supervisor_result.quiesced.cleanup.cancellation.is_some();
        let lifecycle_cancellation = if let Some(owner) = _lifecycle_cancel.take() {
            drop(owner);
            ResourceJoinStatus::Joined
        } else {
            ResourceJoinStatus::NotApplicable
        };
        let supervisor_result =
            resources.finish(supervisor_result, cancelled, lifecycle_cancellation);
        let cleanup = supervisor_result.quiesced.cleanup.clone();
        if !cleanup.proves_quiescence() {
            cancellation.observe_cleanup(temper_process_containment::CleanupSnapshot::Blocked {
                trigger: cleanup.containment.trigger(),
                phase: temper_process_containment::CleanupPhase::Reap,
                message: "one or more attempt resource threads could not be joined".to_string(),
                survivors: cleanup.containment.observed_survivors().to_vec(),
                omitted_survivors: cleanup.containment.omitted_survivors(),
            });
        }
        let _ = cancellation.record_cleanup(cleanup.clone());
        if !cleanup.proves_quiescence() {
            return Err(AgentRunError::transient(
                "agent cleanup did not prove recursive emptiness and endpoint joins",
            ));
        }
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
}

fn bounded_forge_future(
    future: AgentForgeContextFuture,
    timeout: Duration,
) -> AgentForgeContextFuture {
    Box::pin(async move {
        match skein::time::timeout(temper_worker_io::engine_now(), timeout, future).await {
            Ok(result) => result,
            Err(_) => Err(temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable),
        }
    })
}

fn bounded_submit_future(future: SubmitForPrFuture, timeout: Duration) -> SubmitForPrFuture {
    Box::pin(async move {
        match skein::time::timeout(temper_worker_io::engine_now(), timeout, future).await {
            Ok(response) => response,
            Err(_) => SubmitForPrResponse::rejected(format!(
                "submit_for_pr exceeded the generic tool deadline of {:.3}s",
                timeout.as_secs_f64()
            )),
        }
    })
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
