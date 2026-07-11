// SPDX-License-Identifier: MPL-2.0

//! HTTP at the engine boundary.
//!
//! Server side: inbound HTTP requests become `<io-event-completion>`s carrying
//! an [`HttpResponder`]; the machine answers by emitting a respond request,
//! which the executor fulfils through the responder. Connection handling,
//! parsing, and keep-alive all live in the shell (skein's HTTP/1.1
//! listener); the machine only ever sees plain data.
//!
//! Client side: an outbound HTTP call is a single request from the machine's
//! perspective; the executor performs it on skein's pooled HTTP/1.1
//! client and submits the outcome as a completion.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use skein::http::h1::http_client::{ClientError, HttpClient, HttpClientConfig, RedirectPolicy};
use skein::http::h1::{
    Http1Listener, Http1ListenerConfig, Method, Request as H1Request, Response as H1Response,
};
use skein::net::tcp::TcpListenerBuilder;
use skein::runtime::RuntimeHandle;
use skein::server::shutdown::ShutdownSignal;

use crate::queue::{CqSender, OneshotSender, oneshot};
use crate::spawn::Spawner;

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
    /// Whether the connection task is still waiting for this response.
    pub fn is_open(&self) -> bool {
        self.0.is_receiver_alive()
    }

    /// Send the response to the waiting connection task.
    pub fn respond(self, response: HttpResponseData) {
        let _ = self.0.send(response);
    }

    /// Send only if the connection is still waiting, reporting delivery loss.
    pub fn try_respond(self, response: HttpResponseData) -> bool {
        self.0.send(response)
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

/// A boxed h1 request handler bridging connections to an engine queue — what
/// [`h1_completion_handler`] builds. The TCP listener path and in-memory
/// simulation gateways both serve connections through one of these, so
/// simulated traffic exercises the exact production conversion.
pub type H1CompletionHandler = Arc<
    dyn Fn(H1Request) -> std::pin::Pin<Box<dyn std::future::Future<Output = H1Response> + Send>>
        + Send
        + Sync,
>;

/// Build the handler that converts one parsed h1 request into an engine
/// completion via `to_completion`, submits it to the queue, and waits for the
/// machine's response through the responder. If the engine is gone (queue
/// closed or responder dropped), the request answers 500.
pub fn h1_completion_handler<C, F>(cq: CqSender<C>, to_completion: F) -> H1CompletionHandler
where
    C: Send + 'static,
    F: Fn(HttpRequestData, HttpResponder) -> C + Send + Sync + 'static,
{
    Arc::new(move |request: H1Request| {
        let cq = cq.clone();
        let data = HttpRequestData {
            method: request.method.as_str().to_string(),
            uri: request.uri,
            headers: request.headers,
            body: request.body,
        };
        let (reply_tx, reply_rx) = oneshot();
        let completion = to_completion(data, HttpResponder(reply_tx));
        Box::pin(async move {
            if cq.send(completion).is_err() {
                return H1Response::new(500, "Internal Server Error", Vec::new());
            }
            match reply_rx.recv().await {
                Some(response) => {
                    let reason = skein::http::h1::types::default_reason(response.status);
                    let mut h1 = H1Response::new(response.status, reason, response.body);
                    for (name, value) in response.headers {
                        h1 = h1.with_header(name, value);
                    }
                    h1
                }
                None => H1Response::new(500, "Internal Server Error", Vec::new()),
            }
        })
    })
}

/// Bind and serve HTTP/1.1 on `bind`. Every inbound request is handled by
/// [`h1_completion_handler`] semantics against the given queue.
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
    let handler = h1_completion_handler(cq, to_completion);
    // The skein runtime (our asupersync 0.2.4 fork) accepts any Host header by default;
    // it predates the 421-by-default host policy introduced upstream in 0.2.5+.
    // Engine services are reached by IP, localhost, or hostname interchangeably,
    // authenticate at the application layer (webhook HMAC, job protocol), and
    // never derive absolute URLs from Host, so accept-all is the correct policy.
    let config = Http1ListenerConfig::default();
    // On Unix, fixed-port demos and local daemon restarts should not fail just
    // because the previous listener left accepted connections in TCP teardown.
    // Keep SO_REUSEPORT off so an active listener still protects the address.
    let listener_builder = TcpListenerBuilder::new(bind);
    #[cfg(unix)]
    let listener_builder = listener_builder.reuse_addr(true);
    let tcp_listener = listener_builder.bind().await?;
    let listener = Http1Listener::from_listener(
        tcp_listener,
        move |request: H1Request| handler(request),
        config,
    );

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
    let config = HttpClientConfig {
        redirect_policy: RedirectPolicy::None,
        ..HttpClientConfig::default()
    };
    Arc::new(HttpClient::with_config(config))
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
///
/// The skein 0.2.4 client request is not `Cx`-threaded; cancellation of the
/// calling task still tears down the in-flight future. When the client grows
/// cancel-awareness, this takes an explicit `&Cx` — never an ambient one.
pub async fn http_call(client: &HttpClient, call: HttpCall) -> HttpCallResult {
    let method = parse_method(&call.method)?;
    let response = client
        .request(method, &call.url, call.headers, call.body)
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
        http_call(&self.client, call).await
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
    spawner: &dyn Spawner,
    client: Arc<HttpClient>,
    call: HttpCall,
    cq: &CqSender<C>,
    to_completion: F,
) where
    C: Send + 'static,
    F: FnOnce(HttpCallResult) -> C + Send + 'static,
{
    let cq = cq.clone();
    spawner.spawn_with_cx(move |_cx| async move {
        let result = http_call(&client, call).await;
        let _ = cq.send(to_completion(result));
    });
}
