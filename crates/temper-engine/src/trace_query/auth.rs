// SPDX-License-Identifier: MPL-2.0

use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;

use super::ApiError;

const TOKEN_COMPARISON_KEY: &[u8] = b"temper-agent-trace-query-token-v1";

pub(super) fn authorize(
    headers: &[(String, String)],
    expected: &SecretString,
) -> Result<(), ApiError> {
    match presented_bearer(headers) {
        PresentedBearer::Missing => Err(ApiError::Unauthorized),
        PresentedBearer::Invalid => Err(ApiError::Forbidden),
        PresentedBearer::Token(token) if !token_matches(expected.expose_secret(), token) => {
            Err(ApiError::Forbidden)
        }
        PresentedBearer::Token(_) => Ok(()),
    }
}

enum PresentedBearer<'a> {
    Missing,
    Invalid,
    Token(&'a str),
}

fn presented_bearer(headers: &[(String, String)]) -> PresentedBearer<'_> {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str());
    let Some(value) = values.next() else {
        return PresentedBearer::Missing;
    };
    if values.next().is_some() {
        return PresentedBearer::Invalid;
    }
    let Some((scheme, token)) = value.split_once(' ') else {
        return PresentedBearer::Invalid;
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return PresentedBearer::Invalid;
    }
    PresentedBearer::Token(token)
}

fn token_matches(expected: &str, presented: &str) -> bool {
    type TokenMac = Hmac<Sha256>;
    let mut expected_mac =
        TokenMac::new_from_slice(TOKEN_COMPARISON_KEY).expect("fixed HMAC key is valid");
    expected_mac.update(expected.as_bytes());
    let expected_tag = expected_mac.finalize().into_bytes();

    let mut presented_mac =
        TokenMac::new_from_slice(TOKEN_COMPARISON_KEY).expect("fixed HMAC key is valid");
    presented_mac.update(presented.as_bytes());
    presented_mac.verify_slice(&expected_tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_and_parsing_reject_ambiguous_values() {
        assert!(token_matches("secret", "secret"));
        assert!(!token_matches("secret", "wrong"));
        assert!(matches!(
            presented_bearer(&[("Authorization".to_string(), "Bearer secret".to_string())]),
            PresentedBearer::Token("secret")
        ));
        assert!(matches!(
            presented_bearer(&[
                ("Authorization".to_string(), "Bearer secret".to_string()),
                ("authorization".to_string(), "Bearer secret".to_string()),
            ]),
            PresentedBearer::Invalid
        ));
    }
}
