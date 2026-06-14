// SPDX-License-Identifier: MPL-2.0

//! Seeded randomized robustness battery for the skein h1 codec.
//!
//! No external fuzzer: a deterministic xorshift PRNG drives two properties the
//! decoder must satisfy on *any* input — well-formed or hostile:
//!   1. it never panics (only `Ok`/`Err`), including on byte-at-a-time delivery;
//!   2. feeding a request whole vs. split at every boundary yields the same
//!      decode outcome (framing must not depend on TCP segmentation).
//!
//! Deterministic by seed so failures reproduce exactly.

use skein::bytes::BytesMut;
use skein::codec::Decoder;
use skein::http::h1::Http1Codec;

/// Tiny deterministic PRNG (xorshift64*). Seeded; no `rand` dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 33) as u8
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// A pool of tokens biased toward HTTP-significant bytes, so random inputs land
/// near real framing far more often than uniform noise would.
const TOKENS: &[&[u8]] = &[
    b"GET",
    b"POST",
    b" ",
    b"/",
    b"HTTP/1.1",
    b"HTTP/1.0",
    b"\r\n",
    b"\r",
    b"\n",
    b"Host: x",
    b"Content-Length: ",
    b"Transfer-Encoding: chunked",
    b":",
    b";",
    b"chunked",
    b"0",
    b"5",
    b"+5",
    b"hello",
    b"\r\n\r\n",
    b"a",
    b"\t",
    b"%",
    b"==",
];

/// Build a random request-ish byte string from the token pool plus noise.
fn random_input(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    let pieces = 1 + rng.below(24);
    for _ in 0..pieces {
        if rng.below(4) == 0 {
            out.push(rng.byte());
        } else {
            out.extend_from_slice(TOKENS[rng.below(TOKENS.len())]);
        }
    }
    out
}

/// Decode `raw` delivered in one shot. Returns Ok(found-a-frame?) or the error
/// string; never panics.
fn decode_whole(raw: &[u8]) -> Result<bool, String> {
    let mut codec = Http1Codec::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(raw);
    codec
        .decode(&mut buf)
        .map(|o| o.is_some())
        .map_err(|e| e.to_string())
}

/// Decode `raw` delivered one byte at a time, re-decoding after each push.
fn decode_byte_at_a_time(raw: &[u8]) -> Result<bool, String> {
    let mut codec = Http1Codec::new();
    let mut buf = BytesMut::new();
    for &b in raw {
        buf.extend_from_slice(&[b]);
        match codec.decode(&mut buf) {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(false)
}

#[test]
fn decoder_never_panics_on_random_input() {
    // Several seeds × many cases; any panic fails the test loudly.
    for seed in [1u64, 0x9E37_79B9_7F4A_7C15, 0xD1B5_4A32_D192_ED03, 42, 7] {
        let mut rng = Rng(seed);
        for _ in 0..5_000 {
            let input = random_input(&mut rng);
            // Both delivery strategies must merely not panic.
            let _ = decode_whole(&input);
            let _ = decode_byte_at_a_time(&input);
        }
    }
}

#[test]
fn whole_vs_byte_at_a_time_agree_on_acceptance() {
    // Framing must not depend on how the bytes were segmented: a request that
    // decodes whole must also decode byte-at-a-time, and vice versa. (We compare
    // the accept/reject/incomplete *category*, not error identity.)
    #[derive(PartialEq, Eq, Debug)]
    enum Outcome {
        Accepted,
        Incomplete,
        Rejected,
    }
    fn categorize(r: Result<bool, String>) -> Outcome {
        match r {
            Ok(true) => Outcome::Accepted,
            Ok(false) => Outcome::Incomplete,
            Err(_) => Outcome::Rejected,
        }
    }

    for seed in [3u64, 0xABCD_1234_5678_9F01, 99, 0x5DEE_CE66_D000_0001] {
        let mut rng = Rng(seed);
        for _ in 0..3_000 {
            let input = random_input(&mut rng);
            let whole = categorize(decode_whole(&input));
            let split = categorize(decode_byte_at_a_time(&input));
            // An incomplete whole-decode may become accepted/rejected only once
            // more bytes arrive — but here the same bytes are fully delivered in
            // both modes, so categories must match.
            assert_eq!(
                whole, split,
                "segmentation changed decode outcome for input {input:02x?}"
            );
        }
    }
}
