// SPDX-License-Identifier: MPL-2.0

//! RFC 9112 request-framing / smuggling battery for the skein
//! h1 codec (skein, our MIT asupersync 0.2.4 fork). These are black-box
//! tests over the public `Http1Codec` decoder: they feed crafted request bytes
//! and assert that ambiguous or malformed framing is rejected rather than
//! silently accepted (which is how request smuggling slips through).
//!
//! The fork hardened two parsers beyond stock 0.2.4 (chunk-size and
//! Content-Length strictness, per RFC 9112 §6.2/§7.1); these lock that in.

use skein::bytes::BytesMut;
use skein::codec::Decoder;
use skein::http::h1::{Http1Codec, Request};

/// Decode a complete request from a single buffer.
fn decode(raw: &[u8]) -> Result<Option<Request>, String> {
    let mut codec = Http1Codec::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(raw);
    codec.decode(&mut buf).map_err(|e| e.to_string())
}

/// Assert the request bytes are rejected (decode errors).
fn assert_rejected(raw: &[u8], what: &str) {
    match decode(raw) {
        Err(_) => {}
        Ok(_) => panic!("expected rejection of {what}, but it was accepted"),
    }
}

/// Assert the request bytes decode into a complete request.
fn assert_accepted(raw: &[u8], what: &str) -> Request {
    match decode(raw) {
        Ok(Some(req)) => req,
        Ok(None) => panic!("expected a complete request for {what}, got incomplete"),
        Err(e) => panic!("expected {what} to be accepted, but it was rejected: {e}"),
    }
}

// ── Content-Length / Transfer-Encoding conflicts ────────────────────────────

#[test]
fn rejects_content_length_and_transfer_encoding_together() {
    // RFC 9112 §6.1: CL + TE present together is the classic smuggling vector.
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        "Content-Length + Transfer-Encoding",
    );
}

#[test]
fn rejects_duplicate_content_length() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello",
        "duplicate Content-Length",
    );
}

#[test]
fn rejects_duplicate_transfer_encoding() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        "duplicate Transfer-Encoding",
    );
}

// ── Content-Length strictness (RFC 9112 §6.2: 1*DIGIT) ──────────────────────

#[test]
fn rejects_content_length_with_plus_sign() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: +5\r\n\r\nhello",
        "Content-Length: +5",
    );
}

#[test]
fn rejects_content_length_with_interior_space() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5 0\r\n\r\nhello00000",
        "Content-Length: 5 0",
    );
}

#[test]
fn rejects_hex_content_length() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 0x5\r\n\r\nhello",
        "Content-Length: 0x5",
    );
}

#[test]
fn accepts_plain_content_length() {
    let req = assert_accepted(
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello",
        "Content-Length: 5",
    );
    assert_eq!(req.body, b"hello");
}

// ── Transfer-Encoding token strictness ──────────────────────────────────────

#[test]
fn rejects_transfer_encoding_chunked_plus_other() {
    // "chunked, gzip" — only bare `chunked` is supported; anything else is a
    // length-confusion risk.
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked, gzip\r\n\r\n0\r\n\r\n",
        "Transfer-Encoding: chunked, gzip",
    );
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: identity\r\n\r\n",
        "Transfer-Encoding: identity",
    );
}

// ── Chunk-size strictness (RFC 9112 §7.1: 1*HEXDIG) ─────────────────────────

#[test]
fn rejects_chunk_size_with_plus_sign() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n+5\r\nhello\r\n0\r\n\r\n",
        "chunk-size +5",
    );
}

#[test]
fn rejects_chunk_size_with_leading_space() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n 5\r\nhello\r\n0\r\n\r\n",
        "chunk-size ' 5'",
    );
}

#[test]
fn rejects_chunk_size_with_trailing_space() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5 \r\nhello\r\n0\r\n\r\n",
        "chunk-size '5 '",
    );
}

#[test]
fn rejects_non_hex_chunk_size() {
    assert_rejected(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nxy\r\nhello\r\n0\r\n\r\n",
        "chunk-size 'xy'",
    );
}

#[test]
fn accepts_valid_chunked_body() {
    let req = assert_accepted(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        "valid chunked body",
    );
    assert_eq!(req.body, b"hello");
}

#[test]
fn accepts_chunk_extension_after_semicolon() {
    // chunk-ext is opaque and ignored; the hex size before `;` still parses.
    let req = assert_accepted(
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5;name=value\r\nhello\r\n0\r\n\r\n",
        "chunk-ext",
    );
    assert_eq!(req.body, b"hello");
}

// ── Header-injection / CRLF strictness ──────────────────────────────────────

#[test]
fn rejects_bare_lf_in_header_value() {
    // A literal LF inside a header value is a header-injection vector.
    assert_rejected(
        b"GET / HTTP/1.1\r\nHost: x\r\nX-Bad: a\nb\r\n\r\n",
        "bare LF in header value",
    );
}

#[test]
fn rejects_empty_header_name() {
    assert_rejected(
        b"GET / HTTP/1.1\r\nHost: x\r\n: value\r\n\r\n",
        "empty header name",
    );
}

#[test]
fn rejects_obs_fold_continuation() {
    // RFC 9112 §5.2 deprecates obs-fold; a continuation line has no colon.
    assert_rejected(
        b"GET / HTTP/1.1\r\nHost: x\r\nX-Folded: a\r\n  continued\r\n\r\n",
        "obs-fold continuation line",
    );
}

#[test]
fn accepts_minimal_get() {
    assert_accepted(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", "minimal GET");
}
