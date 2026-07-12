// SPDX-License-Identifier: MPL-2.0

//! Worker-side client for authenticated, assignment-scoped context reads.

use skein::cx::Cx;
use temper_protocol_worker::{
    ContextOutcome, FetchContext, ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult,
    WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};

use crate::transport::{HttpTransport, Transport};

/// A context client bound to one worker's currently active job.
///
/// The caller supplies only a closed-vocabulary read operation. This client
/// adds worker/job identity and passes the worker-pool credential through the
/// selected transport (HTTP bearer header or co-resident auth metadata).
pub struct ForgeContextClient<T: Transport> {
    transport: T,
    worker_id: String,
    job_id: String,
    auth: Option<WorkerAuth>,
}

impl<T: Transport> ForgeContextClient<T> {
    pub fn new(
        transport: T,
        worker_id: impl Into<String>,
        job_id: impl Into<String>,
        auth: Option<WorkerAuth>,
    ) -> Self {
        Self {
            transport,
            worker_id: worker_id.into(),
            job_id: job_id.into(),
            auth,
        }
    }

    pub async fn fetch(
        &self,
        cx: Cx,
        operation: ForgeContextOperation,
    ) -> Result<ForgeContextResult, ContextClientError> {
        let request = FetchContext::new(&self.worker_id, &self.job_id, operation);
        let response = self
            .transport
            .send(
                cx,
                WorkerProtocolMessage::FetchContext(request),
                self.auth.clone(),
            )
            .await
            .map_err(ContextClientError::Transport)?
            .ok_or_else(|| ContextClientError::Protocol("empty context response".to_string()))?;
        let WorkerProtocolMessage::ContextResponse(response) = response else {
            return Err(ContextClientError::Protocol(
                "daemon returned a non-context protocol message".to_string(),
            ));
        };
        if response.protocol_version != WORKER_PROTOCOL_VERSION
            || response.worker_id != self.worker_id
            || response.job_id != self.job_id
        {
            return Err(ContextClientError::Protocol(
                "context response identity mismatch".to_string(),
            ));
        }
        match response.outcome {
            ContextOutcome::Success { result } => Ok(result),
            ContextOutcome::Error { code } => Err(ContextClientError::Daemon(code)),
        }
    }
}

/// Split-deployment client using `POST /v1/message` and bearer authentication.
pub type HttpForgeContextClient = ForgeContextClient<HttpTransport>;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ContextClientError {
    #[error("context transport failed: {0}")]
    Transport(String),
    #[error("invalid context protocol response: {0}")]
    Protocol(String),
    #[error("daemon rejected context read: {0:?}")]
    Daemon(ForgeContextErrorCode),
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    use super::*;
    use temper_protocol_worker::{ContextResponse, ForgeGetItemOperation};

    type Sent = Option<(WorkerProtocolMessage, Option<WorkerAuth>)>;

    #[derive(Clone, Default)]
    struct RecordingTransport {
        sent: Arc<Mutex<Sent>>,
    }

    impl Transport for RecordingTransport {
        fn send(
            &self,
            _cx: Cx,
            message: WorkerProtocolMessage,
            auth: Option<WorkerAuth>,
        ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
            *self.sent.lock().expect("recording transport") = Some((message.clone(), auth));
            async move {
                let WorkerProtocolMessage::FetchContext(request) = message else {
                    return Err("unexpected request".to_string());
                };
                Ok(Some(WorkerProtocolMessage::ContextResponse(
                    ContextResponse::error(&request, ForgeContextErrorCode::NotFound),
                )))
            }
        }
    }

    #[test]
    fn client_adds_assignment_identity_and_transport_auth() {
        temper_engine_io::block_on_with(move |cx, _handle| async move {
            let transport = RecordingTransport::default();
            let sent = Arc::clone(&transport.sent);
            let client = ForgeContextClient::new(
                transport,
                "worker-a",
                "job-283",
                Some(WorkerAuth::bearer("pool-secret")),
            );
            let operation = ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
                repo: "ai/temper".to_string(),
                number: 283,
                artifact_type: None,
                include_comments: false,
            });
            assert_eq!(
                client.fetch(cx, operation.clone()).await,
                Err(ContextClientError::Daemon(ForgeContextErrorCode::NotFound))
            );
            let (message, auth) = sent.lock().expect("recording transport").clone().unwrap();
            let WorkerProtocolMessage::FetchContext(request) = message else {
                panic!("expected fetch-context request")
            };
            assert_eq!(request.worker_id, "worker-a");
            assert_eq!(request.job_id, "job-283");
            assert_eq!(request.operation, operation);
            assert_eq!(auth.expect("auth metadata").expose_bearer(), "pool-secret");
        });
    }
}
