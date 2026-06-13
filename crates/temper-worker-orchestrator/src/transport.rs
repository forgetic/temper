// SPDX-License-Identifier: MPL-2.0

//! The worker→daemon transport seam.
//!
//! [`Transport`] is the one capability the [`WorkerShell`](crate::worker_shell::WorkerShell)
//! needs to reach the daemon: deliver one worker-protocol message and yield the
//! daemon's reply. The split deployment uses [`HttpTransport`] (POST over the
//! skein HTTP client); the unified single-process mode supplies an in-process
//! transport that hands the message straight to a co-resident `DaemonCore` over
//! an in-memory channel. The protocol (`temper-worker-protocol`) is identical
//! across both — only the carrier under it changes.

use std::future::Future;
use std::sync::Arc;

use skein::cx::Cx;
use skein::http::h1::http_client::HttpClient;
use temper_worker_io_engine::{HttpCall, HttpResponseData, build_http_client, http_call};
use temper_worker_protocol::WorkerProtocolMessage;

/// Delivers worker→daemon protocol messages and yields the daemon's replies.
///
/// The reply contract matches the daemon's `DaemonCore::handle`:
/// `Ok(None)` for an empty/204 reply, `Ok(Some(_))` for a message, `Err` for a
/// transport failure (which the worker machine treats as a retryable I/O error).
pub trait Transport: Send + Sync + 'static {
    fn send(
        &self,
        cx: Cx,
        message: WorkerProtocolMessage,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send;
}

/// The split-deployment transport: POST each message to `<daemon_url>/v1/message`
/// over the skein HTTP client.
pub struct HttpTransport {
    http: Arc<HttpClient>,
    endpoint: String,
}

impl HttpTransport {
    /// `daemon_url` is the base daemon URL; messages post to
    /// `<daemon_url>/v1/message`.
    pub fn new(daemon_url: &str) -> Self {
        Self {
            http: build_http_client(),
            endpoint: format!("{}/v1/message", daemon_url.trim_end_matches('/')),
        }
    }
}

impl Transport for HttpTransport {
    fn send(
        &self,
        cx: Cx,
        message: WorkerProtocolMessage,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let http = Arc::clone(&self.http);
        let call = HttpCall {
            method: "POST".to_string(),
            url: self.endpoint.clone(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(&message).unwrap_or_default(),
        };
        async move {
            match http_call(&cx, &http, call).await {
                Ok(response) => decode_reply(response),
                Err(error) => Err(error),
            }
        }
    }
}

/// Decode a daemon HTTP response into the worker-protocol reply the machine
/// expects: `Ok(None)` for 204/empty, `Ok(Some(message))` for a 200 JSON body,
/// `Err` for a non-success status or malformed JSON.
fn decode_reply(response: HttpResponseData) -> Result<Option<WorkerProtocolMessage>, String> {
    match response.status {
        204 => Ok(None),
        200 => {
            if response.body.is_empty() {
                return Ok(None);
            }
            serde_json::from_slice::<WorkerProtocolMessage>(&response.body)
                .map(Some)
                .map_err(|error| {
                    let body = String::from_utf8_lossy(&response.body);
                    format!(
                        "daemon response was not valid worker protocol JSON: {error}; body: {body}"
                    )
                })
        }
        status => {
            let body = String::from_utf8_lossy(&response.body);
            Err(format!("daemon returned HTTP {status}: {body}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &[u8]) -> HttpResponseData {
        HttpResponseData {
            status,
            headers: Vec::new(),
            body: body.to_vec(),
        }
    }

    #[test]
    fn decode_204_is_none() {
        assert_eq!(decode_reply(response(204, b"")), Ok(None));
    }

    #[test]
    fn decode_200_empty_is_none() {
        assert_eq!(decode_reply(response(200, b"")), Ok(None));
    }

    #[test]
    fn decode_200_message_parses() {
        let release = serde_json::json!({
            "type": "release",
            "protocol_version": temper_worker_protocol::WORKER_PROTOCOL_VERSION,
            "worker_id": "w1",
            "job_id": "j1",
            "disposition": "accepted",
            "message": null,
        });
        let bytes = serde_json::to_vec(&release).unwrap();
        let decoded = decode_reply(response(200, &bytes)).expect("decodes");
        assert!(matches!(decoded, Some(WorkerProtocolMessage::Release(_))));
    }

    #[test]
    fn decode_non_success_is_err() {
        let error = decode_reply(response(500, b"boom")).expect_err("error status");
        assert!(error.contains("HTTP 500"));
        assert!(error.contains("boom"));
    }

    #[test]
    fn decode_malformed_200_is_err() {
        let error = decode_reply(response(200, b"not json")).expect_err("bad json");
        assert!(error.contains("not valid worker protocol JSON"));
    }
}
