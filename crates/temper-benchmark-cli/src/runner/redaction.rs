// SPDX-License-Identifier: MPL-2.0

//! Fail-closed handling for provider credentials at the artifact boundary.

use serde::Serialize;
use serde::de::DeserializeOwned;
use temper_protocol_activity::BlobAttachmentV1;

use super::BenchmarkRunError;

#[derive(Default)]
pub(super) struct SecretRedactor {
    values: Vec<String>,
}

impl SecretRedactor {
    pub(super) fn from_invocation_env(environment: &[(String, String)]) -> Self {
        let mut values = Vec::new();
        for (_, value) in environment {
            push_secret(&mut values, value);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
                collect_json_secrets(&json, None, &mut values);
            }
        }
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        Self { values }
    }

    pub(super) fn redact_text(&self, value: &str) -> String {
        self.values.iter().fold(value.to_string(), |text, secret| {
            text.replace(secret, "[REDACTED]")
        })
    }

    pub(super) fn redacted<T>(
        &self,
        value: &T,
        artifact: &'static str,
    ) -> Result<T, BenchmarkRunError>
    where
        T: Serialize + DeserializeOwned,
    {
        let mut json = serde_json::to_value(value)
            .map_err(|source| BenchmarkRunError::Json { artifact, source })?;
        self.redact_json_value(&mut json);
        serde_json::from_value(json)
            .map_err(|source| BenchmarkRunError::RedactedJson { artifact, source })
    }

    fn redact_json_value(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => *text = self.redact_text(text),
            serde_json::Value::Array(values) => {
                for value in values {
                    self.redact_json_value(value);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values_mut() {
                    self.redact_json_value(value);
                }
            }
            _ => {}
        }
    }

    pub(super) fn ensure_safe_strings<'a>(
        &self,
        values: impl IntoIterator<Item = &'a String>,
        artifact: &'static str,
    ) -> Result<(), BenchmarkRunError> {
        for value in values {
            self.ensure_safe_bytes(value.as_bytes(), artifact)?;
        }
        Ok(())
    }

    pub(super) fn ensure_safe_attachments(
        &self,
        attachments: &[BlobAttachmentV1],
    ) -> Result<(), BenchmarkRunError> {
        for attachment in attachments {
            if let Ok(bytes) = attachment.decode() {
                self.ensure_safe_bytes(&bytes, "canonical trace attachment")?;
            }
        }
        Ok(())
    }

    pub(super) fn ensure_safe_bytes(
        &self,
        bytes: &[u8],
        artifact: &'static str,
    ) -> Result<(), BenchmarkRunError> {
        if self.values.iter().any(|secret| {
            !secret.is_empty()
                && bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
        }) {
            return Err(BenchmarkRunError::SecretArtifact { artifact });
        }
        Ok(())
    }
}

fn collect_json_secrets(value: &serde_json::Value, key: Option<&str>, secrets: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value)
            if key.is_some_and(|key| {
                matches!(
                    key,
                    "api_key"
                        | "access"
                        | "access_token"
                        | "refresh"
                        | "refresh_token"
                        | "token"
                        | "key"
                        | "secret"
                        | "value"
                )
            }) =>
        {
            push_secret(secrets, value);
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_json_secrets(value, key, secrets);
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                collect_json_secrets(value, Some(key), secrets);
            }
        }
        _ => {}
    }
}

fn push_secret(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::SecretRedactor;

    #[test]
    fn extracts_individual_api_key_and_oauth_tokens() {
        let api_key = "secret-api-key";
        let oauth_access = "secret-access-token";
        let oauth_refresh = "secret-refresh-token";
        let redactor = SecretRedactor::from_invocation_env(&[
            (
                "API".to_string(),
                format!(r#"{{"type":"api-key","api_key":"{api_key}"}}"#),
            ),
            (
                "OAUTH".to_string(),
                format!(
                    r#"{{"type":"oauth","access_token":"{oauth_access}","refresh_token":"{oauth_refresh}"}}"#
                ),
            ),
        ]);

        let redacted = redactor.redact_text(&format!(
            "api={api_key} access={oauth_access} refresh={oauth_refresh}"
        ));
        assert_eq!(
            redacted,
            "api=[REDACTED] access=[REDACTED] refresh=[REDACTED]"
        );
    }
}
