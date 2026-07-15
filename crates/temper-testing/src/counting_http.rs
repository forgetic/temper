//! Reusable operation-counting HTTP seam for Forgejo budget tests.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use temper_forge_forgejo::{HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse};

#[derive(Clone, Debug)]
pub struct CountingHttpClient<C> {
    inner: C,
    requests: Arc<Mutex<Vec<HttpRequest>>>,
}

impl<C> CountingHttpClient<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests
            .lock()
            .expect("counting HTTP client mutex poisoned")
            .clone()
    }

    pub fn get_count(&self) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.method == HttpMethod::Get)
            .count()
    }

    pub fn non_get_count(&self) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.method != HttpMethod::Get)
            .count()
    }

    pub fn request_count(&self) -> usize {
        self.requests
            .lock()
            .expect("counting HTTP client mutex poisoned")
            .len()
    }

    /// Checks a request ceiling and includes every observed method/path in the
    /// error so a budget regression can be diagnosed from one assertion.
    pub fn check_budget(
        &self,
        max_gets: usize,
        max_non_gets: usize,
        max_total: usize,
    ) -> Result<(), String> {
        let requests = self.requests();
        let gets = requests
            .iter()
            .filter(|request| request.method == HttpMethod::Get)
            .count();
        let non_gets = requests.len() - gets;
        if gets <= max_gets && non_gets <= max_non_gets && requests.len() <= max_total {
            return Ok(());
        }
        let paths = requests
            .iter()
            .map(|request| format!("{} {}", request.method, request.path))
            .collect::<Vec<_>>()
            .join("\n");
        Err(format!(
            "Forgejo HTTP budget exceeded: GET {gets}/{max_gets}, non-GET {non_gets}/{max_non_gets}, total {}/{}\n{paths}",
            requests.len(),
            max_total
        ))
    }
}

#[async_trait]
impl<C: HttpClient> HttpClient for CountingHttpClient<C> {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests
            .lock()
            .expect("counting HTTP client mutex poisoned")
            .push(request.clone());
        self.inner.execute(request).await
    }
}
