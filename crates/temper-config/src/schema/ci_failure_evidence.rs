use serde::{Deserialize, Serialize};

impl super::ForgeConfig {
    /// `true` when every field is unset, so the section can be omitted entirely.
    pub(super) fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.url.is_none()
            && self.admin.is_none()
            && self.ci_failure_evidence.is_none()
    }
}

/// `[forge.ci_failure_evidence]` — one closed stronger-evidence transport.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeCiFailureEvidenceConfig {
    /// Absolute HTTPS endpoint, or loopback HTTP endpoint for a single-host runner.
    pub endpoint: String,
    /// Authorized issuer identity carried by every signed statement.
    pub issuer: String,
    /// Allowlist of protected producer identities.
    pub protected_producers: Vec<String>,
    /// Named secret used as the endpoint's acquisition bearer credential.
    pub bearer_token: String,
    /// Named secret used to verify each statement's HMAC-SHA256 integrity.
    pub hmac_key: String,
}
