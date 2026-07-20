//! Support for the ignored Forgejo web-UI contract test.

use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use temper_forge_forgejo::{
    EngineHttpClient, HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse,
};

/// One request/response pair observed by the live delegating client.
///
/// Deliberately does not implement `Debug`: login requests contain the fixture
/// password and must never be rendered by a failed assertion.
#[derive(Clone)]
pub struct ContractExchange {
    pub request: HttpRequest,
    pub response: HttpResponse,
}

/// Live HTTP client that delegates every request except the configured REST
/// Actions-runs discovery request. The synthetic 404 selects ADR 0019's
/// password-authenticated adapter while all web-UI traffic remains real.
#[derive(Clone)]
pub struct RestRuns404Client {
    inner: EngineHttpClient,
    rest_runs_path: String,
    exchanges: Arc<Mutex<Vec<ContractExchange>>>,
}

impl RestRuns404Client {
    pub fn new(base_url: &str, rest_runs_path: String) -> Self {
        Self {
            inner: EngineHttpClient::new(base_url),
            rest_runs_path,
            exchanges: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn exchanges(&self) -> Vec<ContractExchange> {
        self.exchanges
            .lock()
            .expect("contract exchange lock")
            .clone()
    }
}

#[async_trait]
impl HttpClient for RestRuns404Client {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let response = if request.method == HttpMethod::Get && request.path == self.rest_runs_path {
            HttpResponse {
                status: 404,
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: json!({ "message": "Not Found" }).to_string(),
            }
        } else {
            self.inner.execute(request.clone()).await?
        };
        self.exchanges
            .lock()
            .expect("contract exchange lock")
            .push(ContractExchange {
                request,
                response: response.clone(),
            });
        Ok(response)
    }
}

pub fn exchange(
    exchanges: &[ContractExchange],
    method: HttpMethod,
    path: &str,
) -> ContractExchange {
    exchanges
        .iter()
        .find(|exchange| exchange.request.method == method && exchange.request.path == path)
        .cloned()
        .unwrap_or_else(|| panic!("expected {} {path} exchange", method.as_str()))
}

pub fn request_header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|cookie| {
        let (key, value) = cookie.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}
