//! The password-authenticated web-UI session: cookie jar, version-dependent
//! login/CSRF handling, redirects and login bounces, run discovery, and the
//! live-view read.

use super::dto::{LiveRunDto, LiveViewDto};
use crate::ci_ui_parse::{
    cookie_header, extract_input_value, extract_run_ids, form_encode, is_login_bounce,
    is_login_redirect, login_succeeded, redirect_location, store_cookies,
};
use crate::config::WebUiCredentials;
use crate::ids::RepoCoord;
use crate::{ForgejoForge, HttpClient, HttpMethod, HttpRequest, HttpResponse};
use std::collections::BTreeMap;
use temper_forge_model::{ForgeError, ForgeResult};

/// Maximum redirects followed for a single web-UI request.
const MAX_REDIRECTS: usize = 8;

/// Cookies the Forgejo session login establishes, in name-sorted order.
#[derive(Clone, Debug, Default)]
pub(super) struct CookieJar {
    cookies: BTreeMap<String, String>,
}

impl CookieJar {
    /// Records any `Set-Cookie` headers from a response into the jar.
    fn store(&mut self, response: &HttpResponse) {
        store_cookies(&mut self.cookies, response);
    }

    /// Returns the `Cookie` request header value, or `None` when empty.
    fn header(&self) -> Option<String> {
        cookie_header(&self.cookies)
    }

    /// Returns the value of Forgejo 7's `_csrf` cookie for `X-Csrf-Token`.
    /// Forgejo 15 omits both the cookie and header.
    fn csrf(&self) -> Option<&str> {
        self.cookies.get("_csrf").map(String::as_str)
    }
}

/// Web-UI session bound to a [`ForgejoForge`] backend and its credentials.
pub(super) struct WebUiClient<'a, C: HttpClient> {
    forge: &'a ForgejoForge<C>,
    credentials: &'a WebUiCredentials,
    jar: CookieJar,
}

impl<'a, C: HttpClient> WebUiClient<'a, C> {
    /// Builds an unauthenticated session; call [`Self::login`] before fetching.
    pub(super) fn new(forge: &'a ForgejoForge<C>, credentials: &'a WebUiCredentials) -> Self {
        Self {
            forge,
            credentials,
            jar: CookieJar::default(),
        }
    }

    /// Issues one raw web-UI request through the HTTP seam (no API prefix/token).
    async fn execute(&mut self, request: HttpRequest) -> ForgeResult<HttpResponse> {
        self.forge.record_provider_request();
        let response = self
            .forge
            .http_client()
            .execute(request)
            .await
            .map_err(crate::error::map_transport_error)?;
        self.jar.store(&response);
        Ok(response)
    }

    /// Performs the version-dependent web-UI login handshake, populating the
    /// cookie jar.
    ///
    /// GET `/user/login` to capture initial cookies and the optional Forgejo 7
    /// CSRF input, then POST the form-encoded credentials. A `200`, a redirect
    /// back to `/user/login`, or a non-redirect error is treated as a failed
    /// login (the password is never echoed into the error).
    pub(super) async fn login(&mut self) -> ForgeResult<()> {
        self.jar = CookieJar::default();
        let page = self
            .execute(self.request(HttpMethod::Get, "/user/login", Vec::new(), None))
            .await?;
        let csrf = extract_input_value(&page.body, "_csrf");

        let mut form = vec![
            ("user_name", self.credentials.username.as_str()),
            ("password", self.credentials.password.as_str()),
            ("remember", "on"),
        ];
        if let Some(csrf) = csrf.as_deref() {
            form.push(("_csrf", csrf));
        }
        let body = form_encode(&form);
        let headers = vec![(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        let response = self
            .execute(self.request(HttpMethod::Post, "/user/login", headers, Some(body)))
            .await?;

        if login_succeeded(&response) {
            Ok(())
        } else {
            Err(ForgeError::Backend(format!(
                "forgejo web-ui login failed (status {})",
                response.status
            )))
        }
    }

    /// Fetches a web-UI path with the cookie jar, following redirects and
    /// re-logging in once on a bounce to the login page.
    async fn fetch(
        &mut self,
        method: HttpMethod,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let response = self
            .fetch_following(method, path, headers.clone(), body.clone())
            .await?;
        if is_login_bounce(&response) {
            self.login().await?;
            return self.fetch_following(method, path, headers, body).await;
        }
        Ok(response)
    }

    /// Fetches a path, following up to [`MAX_REDIRECTS`] redirects.
    ///
    /// A redirect to the login page is a session bounce, not a normal redirect:
    /// it is returned to [`Self::fetch`] so the caller can re-login and retry.
    async fn fetch_following(
        &mut self,
        method: HttpMethod,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let mut current = path.to_string();
        for _ in 0..MAX_REDIRECTS {
            let request = self.request(method, &current, headers.clone(), body.clone());
            let response = self.execute(request).await?;
            match redirect_location(&response) {
                Some(location) if is_login_redirect(&location) => return Ok(response),
                Some(location) => current = location,
                None => return Ok(response),
            }
        }
        Err(ForgeError::Backend(format!(
            "forgejo web-ui: too many redirects fetching {path}"
        )))
    }

    /// Builds a raw web-UI request: a host-relative `path` (no `/api/v1`), the
    /// cookie jar header, and `Accept: text/html` unless overridden.
    fn request(
        &self,
        method: HttpMethod,
        path: &str,
        mut headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> HttpRequest {
        if let Some(cookie) = self.jar.header() {
            headers.push(("Cookie".to_string(), cookie));
        }
        if !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
        {
            headers.push((
                "Accept".to_string(),
                "text/html,application/xhtml+xml".to_string(),
            ));
        }
        HttpRequest {
            method,
            path: path.to_string(),
            query: Vec::new(),
            headers,
            body,
        }
    }

    /// Scrapes the run ids visible on the repository Actions page.
    pub(super) async fn discover_run_ids(&mut self, repo: &RepoCoord) -> ForgeResult<Vec<u64>> {
        let path = format!("/{}/{}/actions", repo.owner, repo.name);
        let response = self.fetch(HttpMethod::Get, &path, Vec::new(), None).await?;
        if !response.is_success() {
            return Err(ForgeError::Backend(format!(
                "forgejo web-ui: actions page for {} returned status {}",
                repo.path_segment(),
                response.status
            )));
        }
        Ok(extract_run_ids(&response.body))
    }

    /// Reads one run's live-view JSON (`POST …/jobs/{job}/attempt/1`).
    ///
    /// Forgejo 15 requires the attempt-qualified route; the formerly canonical
    /// unqualified route resolves to attempt zero and returns `500`. Forgejo
    /// 7.0.x used the unqualified route, so a `404` from the qualified route is
    /// retried once against the legacy shape. Returns `None` only when both
    /// route shapes report the run/job absent; other non-success statuses are
    /// hard errors so a missing verdict never reads as a pass/fail.
    pub(super) async fn run_live_view(
        &mut self,
        repo: &RepoCoord,
        run: u64,
        job: u64,
    ) -> ForgeResult<Option<LiveRunDto>> {
        let legacy_path = format!(
            "/{}/{}/actions/runs/{run}/jobs/{job}",
            repo.owner, repo.name
        );
        let attempt_path = format!("{legacy_path}/attempt/1");
        let mut headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        if let Some(csrf) = self.jar.csrf() {
            headers.push(("X-Csrf-Token".to_string(), csrf.to_string()));
        }
        let body = Some("{\"logCursors\":[]}".to_string());
        let mut response = self
            .fetch(
                HttpMethod::Post,
                &attempt_path,
                headers.clone(),
                body.clone(),
            )
            .await?;
        if response.status == 404 {
            response = self
                .fetch(HttpMethod::Post, &legacy_path, headers, body)
                .await?;
        }
        if response.status == 404 {
            return Ok(None);
        }
        if !response.is_success() {
            return Err(ForgeError::Backend(format!(
                "forgejo web-ui: live view for run {run} returned status {}",
                response.status
            )));
        }
        let dto: LiveViewDto = serde_json::from_str(&response.body).map_err(|error| {
            ForgeError::Backend(format!(
                "forgejo web-ui: failed to decode live view: {error}"
            ))
        })?;
        Ok(Some(dto.state.run))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_jar_stores_and_exposes_csrf() {
        let mut jar = CookieJar::default();
        let mut response = HttpResponse::new(200, "");
        response.headers = vec![
            (
                "Set-Cookie".to_string(),
                "i_like_gitea=abc; Path=/".to_string(),
            ),
            ("set-cookie".to_string(), "_csrf=tok; Path=/".to_string()),
        ];
        jar.store(&response);
        let header = jar.header().unwrap();
        assert!(header.contains("i_like_gitea=abc"));
        assert!(header.contains("_csrf=tok"));
        assert_eq!(jar.csrf(), Some("tok"));
    }
}
