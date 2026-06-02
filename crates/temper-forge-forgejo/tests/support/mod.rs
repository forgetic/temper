#![allow(dead_code)]

//! Offline test support: a hand-rolled executor and a recording mock HTTP
//! client. No test in this crate touches the network.

use async_trait::async_trait;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use temper_forge::{IssueId, PullRequestId, RepositoryId};
use temper_forge_forgejo::{
    CasMode, ForgejoConfig, ForgejoForge, HttpClient, HttpError, HttpRequest, HttpResponse,
};

/// Repository coordinate every offline pull-request test operates on.
pub const OWNER: &str = "acme";
pub const REPO: &str = "widgets";

/// Builds a backend wired to a mock client with default (best-effort) config.
pub fn forge(client: MockHttpClient) -> ForgejoForge<MockHttpClient> {
    forge_with(client, CasMode::BestEffort)
}

/// Builds a backend with an explicit conditional-write mode.
pub fn forge_with(client: MockHttpClient, cas_mode: CasMode) -> ForgejoForge<MockHttpClient> {
    let config =
        ForgejoConfig::new("https://forge.example.com", "test-token").with_cas_mode(cas_mode);
    ForgejoForge::with_client(config, client)
}

/// Builds a backend with web-UI credentials for the CI read fallback (ADR 0019).
pub fn forge_with_web_ui(client: MockHttpClient) -> ForgejoForge<MockHttpClient> {
    let config = ForgejoConfig::new("https://forge.example.com", "test-token")
        .with_web_ui_credentials("ci-reader", "s3cret");
    ForgejoForge::with_client(config, client)
}

/// Returns the backend repository id for the shared `acme/widgets` repository.
pub fn repo_id() -> RepositoryId {
    RepositoryId::new(format!("forgejo:{OWNER}/{REPO}"))
}

/// Returns the backend pull-request id for `acme/widgets#number`.
pub fn pull_id(number: u64) -> PullRequestId {
    PullRequestId::new(format!("forgejo:{OWNER}/{REPO}:pull:{number}"))
}

/// Returns the backend issue id for `acme/widgets#number`.
pub fn issue_id(number: u64) -> IssueId {
    IssueId::new(format!("forgejo:{OWNER}/{REPO}:issue:{number}"))
}

/// Parses a recorded request body as JSON for field-by-field assertions.
///
/// Tests compare JSON values rather than raw strings so they do not depend on
/// serde's object key ordering.
pub fn body_json(request: &HttpRequest) -> serde_json::Value {
    let body = request.body.as_deref().expect("request has a body");
    serde_json::from_str(body).expect("request body is valid json")
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

/// Drives a future to completion on the current thread.
///
/// The mock client never parks, so a single poll is expected to complete.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);

    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("mock-backed forgejo futures should not park in tests"),
    }
}

#[derive(Default)]
struct MockState {
    responses: VecDeque<Result<HttpResponse, HttpError>>,
    recorded: Vec<HttpRequest>,
}

/// Recording HTTP client that replays canned responses in FIFO order.
///
/// Clones share one underlying state, so a test can pass a clone into
/// [`ForgejoForge::with_client`](temper_forge_forgejo::ForgejoForge::with_client)
/// and still inspect recorded requests afterward.
#[derive(Clone, Default)]
pub struct MockHttpClient {
    state: Arc<Mutex<MockState>>,
}

impl MockHttpClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a success/failure response to be returned by the next request.
    pub fn push_result(&self, result: Result<HttpResponse, HttpError>) {
        self.state.lock().unwrap().responses.push_back(result);
    }

    /// Queues a response with an explicit status and body.
    pub fn push_response(&self, status: u16, body: impl Into<String>) {
        self.push_result(Ok(HttpResponse::new(status, body)));
    }

    /// Queues a `200 OK` response carrying an `ETag` validator header.
    ///
    /// The version cache prefers the `ETag` over the weak `updated_at` fallback,
    /// so conditional-update tests use this to control the captured validator.
    pub fn push_response_with_etag(&self, body: impl Into<String>, etag: impl Into<String>) {
        self.push_result(Ok(HttpResponse {
            status: 200,
            headers: vec![("ETag".to_string(), etag.into())],
            body: body.into(),
        }));
    }

    /// Queues a transport-level failure.
    pub fn push_transport_error(&self, message: impl Into<String>) {
        self.push_result(Err(HttpError::Transport(message.into())));
    }

    /// Returns every recorded request, in order.
    pub fn recorded(&self) -> Vec<HttpRequest> {
        self.state.lock().unwrap().recorded.clone()
    }

    /// Returns the most recently recorded request, if any.
    pub fn last_request(&self) -> Option<HttpRequest> {
        self.state.lock().unwrap().recorded.last().cloned()
    }

    /// Returns the number of recorded requests.
    pub fn call_count(&self) -> usize {
        self.state.lock().unwrap().recorded.len()
    }
}

#[async_trait]
impl HttpClient for MockHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut state = self.state.lock().unwrap();
        state.recorded.push(request);
        state
            .responses
            .pop_front()
            .expect("MockHttpClient: no canned response queued for request")
    }
}
