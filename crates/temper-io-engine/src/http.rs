// SPDX-License-Identifier: MPL-2.0

//! HTTP at the engine boundary.
//!
//! Server side: inbound HTTP requests become `<io-event-completion>`s carrying
//! an [`HttpResponder`]; the machine answers by emitting a respond request,
//! which the executor fulfils through the responder. Connection handling,
//! parsing, and keep-alive all live in the shell (asupersync's HTTP/1.1
//! listener); the machine only ever sees plain data.
//!
//! Client side: an outbound HTTP call is a single request from the machine's
//! perspective; the executor performs it on asupersync's pooled HTTP/1.1
//! client and submits the outcome as a completion.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use asupersync::cx::Cx;
use asupersync::http::h1::http_client::{ClientError, HttpClient, HttpClientBuilder};
use asupersync::http::h1::server::HostPolicy;
use asupersync::http::h1::{
    Http1Config, Http1Listener, Http1ListenerConfig, Method, Request as H1Request,
    Response as H1Response,
};
use asupersync::runtime::RuntimeHandle;
use asupersync::server::shutdown::ShutdownSignal;

use crate::queue::{oneshot, CqSender, OneshotSender};

/// One parsed inbound HTTP request, as data.
#[derive(Debug)]
pub struct HttpRequestData {
    pub method: String,
    /// Request URI including any query string, e.g. `/v1/message`.
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// One outbound HTTP response, as data.
#[derive(Debug, Clone)]
pub struct HttpResponseData {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponseData {
    pub fn status_only(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: value.to_string().into_bytes(),
        }
    }
}

/// Capability to answer one specific inbound HTTP request. Carried inside a
/// completion; consumed exactly once by the executor when the machine emits
/// the matching respond request.
pub struct HttpResponder(OneshotSender<HttpResponseData>);

impl std::fmt::Debug for HttpResponder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HttpResponder")
    }
}

impl HttpResponder {
    /// Send the response to the waiting connection task.
    pub fn respond(self, response: HttpResponseData) {
        self.0.send(response);
    }

    /// Build a responder from a raw oneshot, so pure machines can be unit
    /// tested without a server: the test keeps the receiving half and asserts
    /// on what the machine responded.
    pub fn from_oneshot(reply: OneshotSender<HttpResponseData>) -> Self {
        Self(reply)
    }
}

/// A running engine HTTP server.
pub struct EngineHttpServer {
    local_addr: SocketAddr,
    shutdown: ShutdownSignal,
}

impl EngineHttpServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Begin graceful drain (stop accepting, let in-flight requests finish).
    pub fn begin_drain(&self, timeout: Duration) {
        let _ = self.shutdown.begin_drain(timeout);
    }
}

/// Bind and serve HTTP/1.1 on `bind`. Every inbound request is converted to a
/// completion via `to_completion` and submitted to the queue; the connection
/// task then waits for the machine's response through the responder. If the
/// engine is gone (queue closed or responder dropped), the connection answers
/// 500.
pub async fn serve_http<C, F>(
    handle: &RuntimeHandle,
    bind: SocketAddr,
    cq: CqSender<C>,
    to_completion: F,
) -> std::io::Result<EngineHttpServer>
where
    C: Send + 'static,
    F: Fn(HttpRequestData, HttpResponder) -> C + Send + Sync + 'static,
{
    let to_completion = Arc::new(to_completion);
    // Accept any Host header. asupersync 0.3.2 rejects all requests by
    // default (421) unless a host policy is set; engine services are reached
    // by IP, localhost, or hostname interchangeably, authenticate at the
    // application layer (webhook HMAC, job protocol), and never derive
    // absolute URLs from Host — the injection surface the policy guards
    // against does not exist here.
    let config = Http1ListenerConfig::default()
        .http_config(Http1Config::default().host_policy(HostPolicy::AllowAll));
    let listener = Http1Listener::bind_with_config(
        bind,
        move |request: H1Request| {
            let cq = cq.clone();
            let to_completion = Arc::clone(&to_completion);
            async move {
                let data = HttpRequestData {
                    method: request.method.as_str().to_string(),
                    uri: request.uri,
                    headers: request.headers,
                    body: request.body,
                };
                let (reply_tx, reply_rx) = oneshot();
                let completion = to_completion(data, HttpResponder(reply_tx));
                if cq.send(completion).is_err() {
                    return H1Response::new(500, "Internal Server Error", Vec::new());
                }
                match reply_rx.recv().await {
                    Some(response) => {
                        let mut h1 = H1Response::builder(response.status);
                        for (name, value) in response.headers {
                            h1 = h1.header(name, value);
                        }
                        h1.body(response.body).build()
                    }
                    None => H1Response::new(500, "Internal Server Error", Vec::new()),
                }
            }
        },
        config,
    )
    .await?;

    let local_addr = listener.local_addr()?;
    let shutdown = listener.shutdown_signal();
    let run_handle = handle.clone();
    handle.spawn(async move {
        let _ = listener.run(&run_handle).await;
    });

    Ok(EngineHttpServer {
        local_addr,
        shutdown,
    })
}

/// One outbound HTTP call, as data.
#[derive(Debug, Clone)]
pub struct HttpCall {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Outcome of an [`HttpCall`].
pub type HttpCallResult = Result<HttpResponseData, String>;

/// Build the pooled engine HTTP client (no redirects — forge code treats
/// redirects as data, mirroring the previous reqwest configuration).
pub fn build_http_client() -> Arc<HttpClient> {
    Arc::new(HttpClientBuilder::new().no_redirects().build())
}

/// Percent-encode one query component (RFC 3986 unreserved set kept verbatim).
fn encode_query_component(raw: &str, out: &mut String) {
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
}

/// Render query pairs as an encoded query string (no leading `?`).
pub fn encode_query(query: &[(String, String)]) -> String {
    let mut out = String::new();
    for (index, (name, value)) in query.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        encode_query_component(name, &mut out);
        out.push('=');
        encode_query_component(value, &mut out);
    }
    out
}

fn parse_method(method: &str) -> Result<Method, String> {
    match method.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::Get),
        "POST" => Ok(Method::Post),
        "PUT" => Ok(Method::Put),
        "DELETE" => Ok(Method::Delete),
        "PATCH" => Ok(Method::Patch),
        "HEAD" => Ok(Method::Head),
        "OPTIONS" => Ok(Method::Options),
        other => Err(format!("unsupported HTTP method {other}")),
    }
}

/// Perform one HTTP call on the pooled client. Used directly by forge
/// executors running inside engine tasks.
pub async fn http_call(cx: &Cx, client: &HttpClient, call: HttpCall) -> HttpCallResult {
    let method = parse_method(&call.method)?;
    let response = client
        .request(cx, method, &call.url, call.headers, call.body)
        .await
        .map_err(|error: ClientError| error.to_string())?;
    Ok(HttpResponseData {
        status: response.status,
        headers: response.headers,
        body: response.body,
    })
}

/// Minimal JSON-over-HTTP convenience for tests and fixtures: one pooled
/// client, optional token auth, JSON bodies, buffered responses.
#[derive(Clone)]
pub struct JsonClient {
    client: Arc<HttpClient>,
}

impl Default for JsonClient {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonClient {
    pub fn new() -> Self {
        Self {
            client: build_http_client(),
        }
    }

    /// Send one request; returns the buffered response or a transport error.
    pub async fn send(
        &self,
        method: &str,
        url: impl Into<String>,
        token: Option<&str>,
        body: Option<&serde_json::Value>,
    ) -> Result<HttpResponseData, String> {
        let mut headers = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization".to_string(), format!("token {token}")));
        }
        if body.is_some() {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        let call = HttpCall {
            method: method.to_string(),
            url: url.into(),
            headers,
            body: body
                .map(|value| value.to_string().into_bytes())
                .unwrap_or_default(),
        };
        let cx = crate::runtime::ambient_cx();
        http_call(&cx, &self.client, call).await
    }

    /// Send and parse the response body as JSON, panicking with `what` context
    /// on transport or parse errors (test-fixture ergonomics).
    pub async fn send_expect_json(
        &self,
        method: &str,
        url: impl Into<String>,
        token: Option<&str>,
        body: Option<&serde_json::Value>,
        what: &str,
    ) -> (u16, serde_json::Value) {
        let response = self
            .send(method, url, token, body)
            .await
            .unwrap_or_else(|error| panic!("{what} failed to send: {error}"));
        let parsed = if response.body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null)
        };
        (response.status, parsed)
    }
}

/// Blocking facade over [`JsonClient`] for synchronous test fixtures: owns a
/// private engine runtime and runs each request to completion on it.
pub struct BlockingJsonClient {
    runtime: crate::runtime::EngineRuntime,
    client: JsonClient,
}

impl Default for BlockingJsonClient {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockingJsonClient {
    pub fn new() -> Self {
        Self {
            runtime: crate::runtime::build_runtime().expect("engine runtime builds"),
            client: JsonClient::new(),
        }
    }

    /// Send one request synchronously.
    pub fn send(
        &self,
        method: &str,
        url: impl Into<String>,
        token: Option<&str>,
        body: Option<&serde_json::Value>,
    ) -> Result<HttpResponseData, String> {
        let client = self.client.clone();
        let method = method.to_string();
        let url = url.into();
        let token = token.map(str::to_string);
        let body = body.cloned();
        crate::runtime::block_on_runtime(&self.runtime, async move {
            client
                .send(&method, url, token.as_deref(), body.as_ref())
                .await
        })
    }

    /// Send and parse as JSON, panicking with `what` context on failure.
    pub fn send_expect_json(
        &self,
        method: &str,
        url: impl Into<String>,
        token: Option<&str>,
        body: Option<&serde_json::Value>,
        what: &str,
    ) -> (u16, serde_json::Value) {
        let response = self
            .send(method, url, token, body)
            .unwrap_or_else(|error| panic!("{what} failed to send: {error}"));
        let parsed = if response.body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null)
        };
        (response.status, parsed)
    }
}

/// Execute an HTTP call as a detached engine request: spawn, perform, and
/// submit the result as a completion.
pub fn execute_http_call<C, F>(
    handle: &RuntimeHandle,
    client: Arc<HttpClient>,
    call: HttpCall,
    cq: &CqSender<C>,
    to_completion: F,
) where
    C: Send + 'static,
    F: FnOnce(HttpCallResult) -> C + Send + 'static,
{
    let cq = cq.clone();
    handle.spawn_with_cx(move |cx| async move {
        let result = http_call(&cx, &client, call).await;
        let _ = cq.send(to_completion(result));
    });
}
