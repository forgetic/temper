//! Runtime-bound audit comment published at a transition's commit boundary.

use serde::{Deserialize, Serialize};

/// An idempotent audit comment that must exist before a transition completes.
///
/// The marker is a stable, caller-supplied substring (normally an HTML comment
/// such as `<!-- temper:comment-key=plan-validation:job-42 -->`). The executor
/// lists ordinary Forge comments and creates the body only when no comment
/// contains that exact marker. Runtime-generated child references are appended
/// to `body` after every `create_issues` child has its final number.
///
/// Both strings use hex at the serde boundary. A negative-validation audit is
/// persisted inside the source issue's HTML workflow metadata, and an audit
/// marker itself contains `-->`; encoding prevents that nested terminator from
/// truncating the durable create intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionCompletionAudit {
    #[serde(with = "hex_string")]
    pub marker: String,
    #[serde(with = "hex_string")]
    pub body: String,
}

impl TransitionCompletionAudit {
    /// Creates a completion audit from its stable marker and human-facing body.
    pub fn new(marker: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            marker: marker.into(),
            body: body.into(),
        }
    }
}

mod hex_string {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex_encode(value.as_bytes()))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = hex_decode(&encoded).map_err(D::Error::custom)?;
        String::from_utf8(bytes).map_err(D::Error::custom)
    }

    fn hex_encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        encoded
    }

    fn hex_decode(encoded: &str) -> Result<Vec<u8>, &'static str> {
        if encoded.len() % 2 != 0 {
            return Err("hex string has an odd length");
        }
        encoded
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = hex_digit(pair[0]).ok_or("hex string contains a non-hex digit")?;
                let low = hex_digit(pair[1]).ok_or("hex string contains a non-hex digit")?;
                Ok((high << 4) | low)
            })
            .collect()
    }

    fn hex_digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
}
