// SPDX-License-Identifier: MPL-2.0
//! Pure parsing/encoding helpers for the web-UI CI read path (ADR 0019).
//!
//! These functions know the version-sensitive HTML/header/SHA shapes the web UI
//! emits: cookie storage, CSRF/run-id scraping, form encoding, redirect/login
//! classification, and commit-SHA prefix matching. They are split out of
//! [`crate::ci_ui`] to keep each module within the source-size budget and are
//! covered by the unit tests below. No I/O lives here.

use crate::ci_match::Target;
use crate::HttpResponse;
use std::collections::BTreeMap;

/// Records any `Set-Cookie` headers from a response into the jar.
pub(super) fn store_cookies(jar: &mut BTreeMap<String, String>, response: &HttpResponse) {
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case("set-cookie") {
            store_set_cookie(jar, value);
        }
    }
}

/// Splits a single (possibly coalesced) `Set-Cookie` value into the jar.
///
/// Forgejo may coalesce multiple cookies into one header separated by commas;
/// the boundary is a comma followed by a `name=` token, not the comma inside an
/// `Expires` attribute. Only the leading `name=value` of each cookie is kept.
fn store_set_cookie(jar: &mut BTreeMap<String, String>, header: &str) {
    for cookie in split_set_cookie(header) {
        let pair = cookie.split(';').next().unwrap_or("").trim();
        if let Some((name, value)) = pair.split_once('=') {
            if !name.is_empty() {
                jar.insert(name.to_string(), value.to_string());
            }
        }
    }
}

/// Splits a coalesced `Set-Cookie` header at cookie boundaries.
fn split_set_cookie(header: &str) -> Vec<String> {
    let bytes = header.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b',' && looks_like_cookie_start(&header[i + 1..]) {
            parts.push(header[start..i].to_string());
            start = i + 1;
        }
        i += 1;
    }
    parts.push(header[start..].to_string());
    parts
}

/// Heuristic: after a comma, a new cookie begins with `<token>=` (no `;`/`,`).
fn looks_like_cookie_start(rest: &str) -> bool {
    let rest = rest.trim_start();
    match rest.find('=') {
        Some(eq) => !rest[..eq].contains([';', ',']) && !rest[..eq].is_empty(),
        None => false,
    }
}

/// Joins a cookie jar into a `Cookie` request header value, or `None` if empty.
pub(super) fn cookie_header(jar: &BTreeMap<String, String>) -> Option<String> {
    if jar.is_empty() {
        return None;
    }
    Some(
        jar.iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Extracts the `value` of an `<input name="…">` from a login HTML page.
pub(super) fn extract_input_value(html: &str, name: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    for tag in html.split("<input").skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        if !tag.contains(&needle) {
            continue;
        }
        if let Some(rest) = tag.split("value=\"").nth(1) {
            return rest.split('"').next().map(str::to_string);
        }
    }
    None
}

/// Scrapes `…/actions/runs/{id}` links from the repository Actions HTML page.
pub(super) fn extract_run_ids(html: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    for fragment in html.split("/actions/runs/").skip(1) {
        let digits: String = fragment.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse::<u64>() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// Form-encodes `name=value` pairs (`application/x-www-form-urlencoded`).
pub(super) fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", percent_encode(name), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Minimal application/x-www-form-urlencoded percent-encoder.
fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Returns the `Location` of a redirect response, or `None` for a final one.
pub(super) fn redirect_location(response: &HttpResponse) -> Option<String> {
    if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
        return response.header("location").map(str::to_string);
    }
    None
}

/// Whether a POST `/user/login` response indicates a successful login.
///
/// A redirect away from `/user/login` is success; a `200` (the form re-rendered),
/// a redirect back to `/user/login`, or any other non-redirect status is failure.
pub(super) fn login_succeeded(response: &HttpResponse) -> bool {
    if response.status == 200 {
        return false;
    }
    match redirect_location(response) {
        Some(location) => !location.contains("/user/login"),
        None => false,
    }
}

/// Whether a fetched response bounced to the login page (session expired).
pub(super) fn is_login_bounce(response: &HttpResponse) -> bool {
    if response.status == 401 || response.status == 403 {
        return true;
    }
    redirect_location(response)
        .map(|location| location.contains("/user/login"))
        .unwrap_or(false)
}

/// Whether a redirect `Location` is a bounce back to the login page.
pub(super) fn is_login_redirect(location: &str) -> bool {
    location.contains("/user/login")
}

/// Whether a run's commit short-SHA satisfies the target's SHA filter.
///
/// A target with no SHA filter (e.g. PR matched only by number, or an empty
/// target) accepts every run, since the live view does not expose the PR ref. A
/// run with no commit info is also kept rather than silently dropping a verdict
/// the caller asked for.
pub(super) fn commit_matches(short_sha: &str, target: &Target) -> bool {
    let candidates: Vec<&str> = [target.commit_sha.as_deref(), target.pr_head_sha.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    if candidates.is_empty() || short_sha.is_empty() {
        return true;
    }
    candidates
        .iter()
        .any(|sha| sha_prefix_match(sha, short_sha))
}

/// Prefix-compares a full SHA against the live view's short SHA.
fn sha_prefix_match(full: &str, short: &str) -> bool {
    let full = full.to_ascii_lowercase();
    let short = short.to_ascii_lowercase();
    if full == short {
        return true;
    }
    let min = full.len().min(short.len());
    min >= 7 && full[..min] == short[..min]
}

/// Returns the first non-empty string from `values`, or an empty string.
pub(super) fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_forge::PullRequestId;

    #[test]
    fn extracts_csrf_input_value() {
        let html =
            r#"<form><input type="hidden" name="_csrf" value="tok-123"><input name="x"></form>"#;
        assert_eq!(
            extract_input_value(html, "_csrf").as_deref(),
            Some("tok-123")
        );
        assert_eq!(extract_input_value(html, "missing"), None);
    }

    #[test]
    fn stores_single_and_coalesced_set_cookies() {
        let mut jar = BTreeMap::new();
        store_set_cookie(&mut jar, "i_like_gitea=abc; Path=/; HttpOnly");
        store_set_cookie(
            &mut jar,
            "gitea_incredible=def; Expires=Mon, 01 Jan 2030 00:00:00 GMT, _csrf=ghi; Path=/",
        );
        assert_eq!(jar.get("i_like_gitea").map(String::as_str), Some("abc"));
        assert_eq!(jar.get("gitea_incredible").map(String::as_str), Some("def"));
        assert_eq!(jar.get("_csrf").map(String::as_str), Some("ghi"));
    }

    #[test]
    fn store_cookies_reads_set_cookie_headers_case_insensitively() {
        let mut jar = BTreeMap::new();
        let mut response = HttpResponse::new(200, "");
        response.headers = vec![
            (
                "Set-Cookie".to_string(),
                "i_like_gitea=abc; Path=/".to_string(),
            ),
            ("set-cookie".to_string(), "_csrf=tok; Path=/".to_string()),
        ];
        store_cookies(&mut jar, &response);
        let header = cookie_header(&jar).unwrap();
        assert!(header.contains("i_like_gitea=abc"));
        assert!(header.contains("_csrf=tok"));
        assert_eq!(cookie_header(&BTreeMap::new()), None);
    }

    #[test]
    fn extracts_run_ids_from_actions_html() {
        let html = r#"
            <a href="/acme/widgets/actions/runs/12">run 12</a>
            <a href="/acme/widgets/actions/runs/9/jobs/0">job</a>
            <a href="/acme/widgets/actions/runs/12">dup</a>
        "#;
        assert_eq!(extract_run_ids(html), vec![12, 9]);
    }

    #[test]
    fn login_success_and_failure_detection() {
        let mut redirect = HttpResponse::new(302, "");
        redirect.headers = vec![("Location".to_string(), "/".to_string())];
        assert!(login_succeeded(&redirect));

        let mut bounce = HttpResponse::new(302, "");
        bounce.headers = vec![("Location".to_string(), "/user/login?redirect".to_string())];
        assert!(!login_succeeded(&bounce));
        assert!(is_login_bounce(&bounce));
        assert!(is_login_redirect("/user/login?redirect"));

        // A 200 means the form re-rendered (bad credentials) — not a success.
        assert!(!login_succeeded(&HttpResponse::new(200, "<form>")));

        assert!(is_login_bounce(&HttpResponse::new(403, "")));
        assert!(!is_login_bounce(&HttpResponse::new(200, "")));
    }

    #[test]
    fn form_encode_escapes_reserved_characters() {
        let encoded = form_encode(&[("user_name", "a b"), ("password", "p@ss/word")]);
        assert_eq!(encoded, "user_name=a+b&password=p%40ss%2Fword");
    }

    #[test]
    fn commit_match_accepts_prefix_and_empty_target() {
        let mut target = Target::default();
        assert!(commit_matches("abc123", &target));
        target.pr_head_sha = Some("abcdef1234567890".to_string());
        let _ = PullRequestId::new("forgejo:acme/widgets:pull:7");
        assert!(commit_matches("abcdef1234567", &target));
        assert!(!commit_matches("ffffff9999999", &target));
        // A run with no commit info is kept rather than dropped.
        assert!(commit_matches("", &target));
    }
}
