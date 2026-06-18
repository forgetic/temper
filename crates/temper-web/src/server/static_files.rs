// SPDX-License-Identifier: MPL-2.0

//! Static UI file serving.
//!
//! Resolves request paths against the configured UI directory and reads the
//! bytes, with a content type from the extension. Path traversal is rejected so
//! a request can never escape the UI root. The resolution is split from the
//! socket I/O so it is unit-testable against a temp dir.
//!
//! The board entry (`/`) serves `index.html`; `/chat` serves `chat.html`; other
//! paths map straight onto files under the root (`/styles.css`,
//! `/dist/board.js`, …) exactly as the HTML references them.

use std::path::{Path, PathBuf};

/// A resolved static file: its content type and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFile {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Why a static request did not resolve to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticError {
    /// The path escaped the UI root (rejected traversal).
    Forbidden,
    /// No such file under the UI root.
    NotFound,
}

/// Resolve a request path to a file under `ui_dir` and read it.
///
/// `/` → `index.html`, `/chat` → `chat.html`; any other path is taken relative
/// to the root. Returns [`StaticError::Forbidden`] for a traversal attempt and
/// [`StaticError::NotFound`] when the file is absent.
pub fn resolve(ui_dir: &Path, request_path: &str) -> Result<StaticFile, StaticError> {
    let relative = map_route(request_path);
    // Reject any segment that could escape the root before touching the FS.
    if relative.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(StaticError::Forbidden);
    }
    let mut full = PathBuf::from(ui_dir);
    for segment in relative.split('/').filter(|s| !s.is_empty()) {
        full.push(segment);
    }
    // Defence in depth: the resolved path must still live under the root.
    if !full.starts_with(ui_dir) {
        return Err(StaticError::Forbidden);
    }
    match std::fs::read(&full) {
        Ok(bytes) => Ok(StaticFile {
            content_type: content_type_for(&full),
            bytes,
        }),
        Err(_) => Err(StaticError::NotFound),
    }
}

/// Map an HTTP route to a file path relative to the UI root.
#[must_use]
pub fn map_route(request_path: &str) -> String {
    let path = request_path
        .split(['?', '#'])
        .next()
        .unwrap_or(request_path);
    match path {
        "" | "/" | "/index.html" => "index.html".to_string(),
        "/chat" | "/chat.html" => "chat.html".to_string(),
        other => other.trim_start_matches('/').to_string(),
    }
}

/// Content type from a file extension (the small set the UI bundle uses).
#[must_use]
pub fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[path = "static_files_tests.rs"]
mod static_files_tests;
