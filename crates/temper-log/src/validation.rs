// SPDX-License-Identifier: MPL-2.0

//! Safe projection helpers for plan-validation outcome summaries.
//!
//! Validation summaries are the only agent-authored text admitted to the
//! `validation.outcome` observability contract. This module owns the projection
//! used by both the event emitter and the durable Forge audit comment so those
//! two records cannot disagree about whitespace normalization, redaction, or
//! truncation.

use crate::redact::redacted_preview;

/// Character budget for a projected validation summary.
///
/// The budget counts Unicode scalar values, including the trailing ellipsis
/// when truncation is required.
pub const VALIDATION_SUMMARY_PREVIEW_LIMIT: usize = 240;

/// Normalizes, secret-redacts, and character-bounds a validation summary.
///
/// Callers publishing the durable validation-audit comment must use this
/// function rather than maintaining a second projection. The
/// `validation.outcome` emitter applies this same function before either its
/// structured or human projection sees the summary.
pub fn validation_summary_preview(summary: &str) -> String {
    redacted_preview(summary, VALIDATION_SUMMARY_PREVIEW_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::REDACTED;

    #[test]
    fn projection_normalizes_whitespace() {
        assert_eq!(
            validation_summary_preview("  checks pass\n\twith follow-up  "),
            "checks pass with follow-up"
        );
    }

    #[test]
    fn projection_redacts_secret_shaped_text() {
        assert_eq!(
            validation_summary_preview("checks pass; Authorization: Bearer private-value"),
            REDACTED
        );
    }

    #[test]
    fn projection_is_character_bounded() {
        let projected = validation_summary_preview(&"é".repeat(300));
        assert_eq!(projected.chars().count(), VALIDATION_SUMMARY_PREVIEW_LIMIT);
        assert!(projected.ends_with('…'));
    }
}
