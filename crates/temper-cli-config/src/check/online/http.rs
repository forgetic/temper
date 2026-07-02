// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use skein::http::h1::Method;
use skein::http::h1::http_client::{ClientError, HttpClient, HttpClientConfig, RedirectPolicy};
use skein::runtime::reactor::create_reactor;
use skein::runtime::{Runtime, RuntimeBuilder};

pub(super) struct BlockingHttpClient {
    runtime: Runtime,
    client: Arc<HttpClient>,
}

pub(super) struct HttpResponse {
    pub(super) status: u16,
}

impl BlockingHttpClient {
    pub(super) fn new() -> Result<Self, String> {
        let reactor = create_reactor()
            .map_err(|error| format!("creating HTTP runtime reactor failed: {error}"))?;
        let runtime = RuntimeBuilder::current_thread()
            .blocking_threads(1, 4)
            .with_reactor(reactor)
            .build()
            .map_err(|error| format!("building HTTP runtime failed: {error}"))?;
        let config = HttpClientConfig {
            redirect_policy: RedirectPolicy::None,
            ..HttpClientConfig::default()
        };
        Ok(Self {
            runtime,
            client: Arc::new(HttpClient::with_config(config)),
        })
    }

    pub(super) fn get(
        &self,
        url: String,
        headers: Vec<(String, String)>,
    ) -> Result<HttpResponse, String> {
        let client = Arc::clone(&self.client);
        let response = self
            .runtime
            .block_on(async move { client.request(Method::Get, &url, headers, Vec::new()).await })
            .map_err(|error: ClientError| error.to_string())?;
        Ok(HttpResponse {
            status: response.status,
        })
    }
}
