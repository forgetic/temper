//! Reusable operation-counting HTTP seam for Forgejo budget tests.

use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use temper_forge_forgejo::{HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForgejoReadShape {
    /// GETs against issue/PR collection endpoints, including owner search.
    pub candidate_list_requests: usize,
    /// Exact issue/PR representation GETs, excluding dependency endpoints.
    pub exact_artifact_reads: usize,
    /// Native `/dependencies` GETs.
    pub dependency_requests: usize,
    /// Other GET traffic, retained so a warm-idle assertion cannot hide N+1s.
    pub other_reads: usize,
}

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

    /// Returns requests recorded at or after `start`, saturating to an empty
    /// slice when a caller supplies a later index.
    pub fn requests_since(&self, start: usize) -> Vec<HttpRequest> {
        self.requests().into_iter().skip(start).collect()
    }

    /// Classifies Forgejo GET traffic since `start` without treating writes as
    /// reads. Exact artifact and dependency calls are deliberately separate so
    /// warm-cache tests can reject either N+1 shape independently.
    pub fn forgejo_read_shape_since(&self, start: usize) -> ForgejoReadShape {
        let mut shape = ForgejoReadShape::default();
        for request in self.requests_since(start) {
            if request.method != HttpMethod::Get {
                continue;
            }
            if is_dependency_path(&request.path) {
                shape.dependency_requests = shape.dependency_requests.saturating_add(1);
            } else if is_exact_artifact_path(&request.path) {
                shape.exact_artifact_reads = shape.exact_artifact_reads.saturating_add(1);
            } else if is_candidate_list_path(&request.path) {
                shape.candidate_list_requests = shape.candidate_list_requests.saturating_add(1);
            } else {
                shape.other_reads = shape.other_reads.saturating_add(1);
            }
        }
        shape
    }

    /// Normalized method/path counts since `start`, suitable for stable local
    /// benchmark output. Numeric resource segments become `{id}`.
    pub fn normalized_method_path_counts_since(&self, start: usize) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for request in self.requests_since(start) {
            let key = format!(
                "{} {}",
                request.method,
                normalize_resource_path(&request.path)
            );
            let count = counts.entry(key).or_insert(0_usize);
            *count = count.saturating_add(1);
        }
        counts
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

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn is_candidate_list_path(path: &str) -> bool {
    if path == "/api/v1/repos/issues/search" {
        return true;
    }
    let segments = path_segments(path);
    segments.len() == 6
        && segments[..3] == ["api", "v1", "repos"]
        && matches!(segments[5], "issues" | "pulls")
}

fn is_exact_artifact_path(path: &str) -> bool {
    let segments = path_segments(path);
    segments.len() == 7
        && segments[..3] == ["api", "v1", "repos"]
        && matches!(segments[5], "issues" | "pulls")
        && segments[6].bytes().all(|byte| byte.is_ascii_digit())
}

fn is_dependency_path(path: &str) -> bool {
    let segments = path_segments(path);
    segments.len() == 8
        && segments[..3] == ["api", "v1", "repos"]
        && segments[5] == "issues"
        && segments[6].bytes().all(|byte| byte.is_ascii_digit())
        && segments[7] == "dependencies"
}

fn normalize_resource_path(path: &str) -> String {
    let mut normalized = String::new();
    for segment in path_segments(path) {
        normalized.push('/');
        if segment.bytes().all(|byte| byte.is_ascii_digit()) {
            normalized.push_str("{id}");
        } else {
            normalized.push_str(segment);
        }
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    normalized
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct OkClient;

    #[async_trait]
    impl HttpClient for OkClient {
        async fn execute(&self, _request: HttpRequest) -> Result<HttpResponse, HttpError> {
            Ok(HttpResponse::new(200, "[]"))
        }
    }

    fn request(method: HttpMethod, path: &str) -> HttpRequest {
        HttpRequest {
            method,
            path: path.to_string(),
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
        }
    }

    #[test]
    fn forgejo_shape_separates_lists_exact_dependencies_and_writes() {
        let client = CountingHttpClient::new(OkClient);
        crate::block_on(async {
            for request in [
                request(HttpMethod::Get, "/api/v1/repos/issues/search"),
                request(HttpMethod::Get, "/api/v1/repos/acme/widgets/issues"),
                request(HttpMethod::Get, "/api/v1/repos/acme/widgets/pulls/17"),
                request(
                    HttpMethod::Get,
                    "/api/v1/repos/acme/widgets/issues/42/dependencies",
                ),
                request(HttpMethod::Patch, "/api/v1/repos/acme/widgets/issues/42"),
            ] {
                client.execute(request).await.expect("request succeeds");
            }
        });

        assert_eq!(
            client.forgejo_read_shape_since(0),
            ForgejoReadShape {
                candidate_list_requests: 2,
                exact_artifact_reads: 1,
                dependency_requests: 1,
                other_reads: 0,
            }
        );
        assert_eq!(
            client.normalized_method_path_counts_since(0)["GET /api/v1/repos/acme/widgets/issues/{id}/dependencies"],
            1
        );
        assert_eq!(
            client.normalized_method_path_counts_since(0)["PATCH /api/v1/repos/acme/widgets/issues/{id}"],
            1,
            "writes remain visible in method/path output but never enter read shape"
        );
    }
}
