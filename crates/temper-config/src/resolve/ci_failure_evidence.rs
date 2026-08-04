use crate::error::ConfigError;
use crate::resolve_options::ResolveOptions;
use crate::resolved::ForgeCiFailureEvidenceSettings;
use crate::schema::{Credentials, ForgeCiFailureEvidenceConfig};
use crate::secret_refs::{require_secret_payload, resolve_secret_reference};

pub(super) fn resolve_ci_failure_evidence(
    source: &ForgeCiFailureEvidenceConfig,
    credentials: &Credentials,
    options: &ResolveOptions,
) -> Result<ForgeCiFailureEvidenceSettings, ConfigError> {
    let endpoint = source.endpoint.trim().to_string();
    if !valid_endpoint(&endpoint) {
        return Err(ConfigError::invalid(
            "forge.ci_failure_evidence.endpoint must be HTTPS or loopback HTTP and contain no query, fragment, user info, or whitespace",
        ));
    }
    let issuer = validated_identity("forge.ci_failure_evidence.issuer", &source.issuer)?;
    if source.protected_producers.is_empty() {
        return Err(ConfigError::invalid(
            "forge.ci_failure_evidence.protected_producers must contain at least one identity",
        ));
    }
    let mut protected_producers = source
        .protected_producers
        .iter()
        .map(|producer| {
            validated_identity("forge.ci_failure_evidence.protected_producers", producer)
        })
        .collect::<Result<Vec<_>, _>>()?;
    protected_producers.sort();
    protected_producers.dedup();

    let bearer_token = resolve_secret_reference(
        "forge.ci_failure_evidence.bearer_token",
        Some(&source.bearer_token),
        credentials,
        options.validate_secret_references,
    )?
    .expect("an explicit evidence section always carries the field");
    let hmac_key = resolve_secret_reference(
        "forge.ci_failure_evidence.hmac_key",
        Some(&source.hmac_key),
        credentials,
        options.validate_secret_references,
    )?
    .expect("an explicit evidence section always carries the field");
    let bearer_token_value = bearer_token
        .reference
        .available
        .then(|| require_secret_payload("forge.ci_failure_evidence.bearer_token", &bearer_token))
        .transpose()?;
    let hmac_key_value = hmac_key
        .reference
        .available
        .then(|| require_secret_payload("forge.ci_failure_evidence.hmac_key", &hmac_key))
        .transpose()?;

    Ok(ForgeCiFailureEvidenceSettings {
        endpoint,
        issuer,
        protected_producers,
        bearer_token: bearer_token.reference,
        bearer_token_value,
        hmac_key: hmac_key.reference,
        hmac_key_value,
    })
}

fn valid_endpoint(value: &str) -> bool {
    if value.is_empty()
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
        || value.contains(['?', '#'])
    {
        return false;
    }
    let (secure, remainder) = if let Some(remainder) = value.strip_prefix("https://") {
        (true, remainder)
    } else if let Some(remainder) = value.strip_prefix("http://") {
        (false, remainder)
    } else {
        return false;
    };
    let authority = remainder.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    secure || valid_loopback_authority(authority)
}

fn valid_loopback_authority(authority: &str) -> bool {
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    matches!(host, "localhost" | "127.0.0.1")
        && port
            .is_none_or(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

fn validated_identity(field: &str, value: &str) -> Result<String, ConfigError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
    {
        return Err(ConfigError::invalid(format!(
            "{field} must be a non-empty identity of at most 128 ASCII identifier bytes"
        )));
    }
    Ok(value.to_string())
}
