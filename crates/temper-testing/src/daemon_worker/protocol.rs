use std::time::Duration;

use temper_worker_protocol::{
    Capacity, ErrorCode, Poll, Register, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use super::job::execute_job;
use super::{DaemonWorkerConfig, GitIdentity};

/// Runs the worker loop until the stop-file appears.
///
/// `register` -> `poll` -> on `assign` execute the job and send `result` ->
/// re-poll. Transport errors are logged and retried (the e2e driver bounds the
/// run with the stop-file and its own convergence timeout).
pub async fn run(
    cx: &temper_engine_io::Cx,
    config: &DaemonWorkerConfig,
    identity: &GitIdentity,
) -> Result<(), String> {
    let client = temper_engine_io::http::JsonClient::new();
    let endpoint = format!("{}/v1/message", config.daemon_url.trim_end_matches('/'));

    let register = WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: config.worker_id.clone(),
        capabilities: config.capabilities.clone(),
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    });
    match send(&client, &endpoint, &register).await? {
        Some(WorkerProtocolMessage::Error(error)) => {
            return Err(format!(
                "daemon rejected registration: {:?}: {}",
                error.code, error.message
            ));
        }
        _ => tracing::info!(
            target: "temper_testing_daemon_worker",
            worker_id = %config.worker_id,
            "registered worker"
        ),
    }

    while !config.stop_file.exists() {
        let poll = WorkerProtocolMessage::Poll(Poll {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: config.worker_id.clone(),
            free_capacity: 1,
            max_wait_ms: Some(config.poll_wait_ms),
        });
        let response = match send(&client, &endpoint, &poll).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    target: "temper_testing_daemon_worker",
                    %error,
                    "poll failed"
                );
                temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(200)).await;
                continue;
            }
        };
        match response {
            Some(WorkerProtocolMessage::Assign(assign)) => {
                tracing::info!(
                    target: "temper_testing_daemon_worker",
                    job_id = %assign.job_id,
                    repo = %assign.repo,
                    role = %assign.role,
                    artifact_kind = %assign.artifact.kind,
                    artifact_item = %assign.artifact.item,
                    "assigned job"
                );
                let result = execute_job(cx, config, identity, &assign).await;
                tracing::info!(
                    target: "temper_testing_daemon_worker",
                    job_id = %assign.job_id,
                    status = ?result.status,
                    failure = ?result.failure.as_ref().map(|failure| failure.class),
                    "job finished"
                );
                let message = WorkerProtocolMessage::Result(result);
                match send(&client, &endpoint, &message).await {
                    Ok(Some(WorkerProtocolMessage::Release(release))) => tracing::info!(
                        target: "temper_testing_daemon_worker",
                        job_id = %release.job_id,
                        disposition = ?release.disposition,
                        "job released"
                    ),
                    Ok(other) => tracing::warn!(
                        target: "temper_testing_daemon_worker",
                        response = ?other,
                        "unexpected result response"
                    ),
                    Err(error) => {
                        tracing::error!(
                            target: "temper_testing_daemon_worker",
                            %error,
                            "result send failed"
                        )
                    }
                }
            }
            Some(WorkerProtocolMessage::Error(error)) if error.code == ErrorCode::PollTimeout => {}
            Some(other) => {
                tracing::warn!(
                    target: "temper_testing_daemon_worker",
                    response = ?other,
                    "unexpected poll response"
                )
            }
            None => {}
        }
    }

    tracing::info!(
        target: "temper_testing_daemon_worker",
        stop_file = %config.stop_file.display(),
        "stop file present; exiting"
    );
    Ok(())
}

async fn send(
    client: &temper_engine_io::http::JsonClient,
    endpoint: &str,
    message: &WorkerProtocolMessage,
) -> Result<Option<WorkerProtocolMessage>, String> {
    let payload = serde_json::to_value(message)
        .map_err(|error| format!("serializing protocol message failed: {error}"))?;
    let response = client
        .send("POST", endpoint, None, Some(&payload))
        .await
        .map_err(|error| format!("request to {endpoint} failed: {error}"))?;
    if response.status == 204 || response.body.is_empty() {
        return Ok(None);
    }
    if response.status != 200 {
        return Err(format!(
            "daemon returned HTTP {} from {endpoint}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
    }
    serde_json::from_slice(&response.body)
        .map(Some)
        .map_err(|error| format!("daemon response was not valid protocol JSON: {error}"))
}
