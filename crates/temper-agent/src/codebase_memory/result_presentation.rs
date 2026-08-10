use std::collections::BTreeSet;

use temper_agent_core::DecisionAnchorEvidenceV1;
use temper_protocol_activity::GraphCorrelationV1;

use super::MAX_CODEBASE_MEMORY_OUTPUT_BYTES;

/// Model-visible guidance that is deliberately generic: it follows only a
/// complete, bounded result and never carries provider arguments or fields.
pub(super) const DECISION_ANCHOR: &str = "\n\n[Decision anchor: This is a bounded successful targeted current-root result. In a later model turn, use the provider result above with the work-item requirements to select the next dependent refinement, trace, or exact source read.]";

pub(super) struct PresentedResult {
    pub(super) text: String,
    /// The exact bounded provider text shown before any generic anchor suffix.
    /// It stays in-process only while the wrapper derives fingerprints.
    pub(super) provider_text: String,
    pub(super) truncated: bool,
    pub(super) decision_anchor: bool,
}

/// Keeps the complete model-visible result, including its optional anchor,
/// within the existing output limit. A result that needs truncation remains
/// useful discovery, but is intentionally not a decision anchor.
pub(super) fn present_result(
    result: &str,
    correlation: Option<&GraphCorrelationV1>,
) -> PresentedResult {
    let max_result_bytes = if correlation.is_some() {
        MAX_CODEBASE_MEMORY_OUTPUT_BYTES.saturating_sub(DECISION_ANCHOR.len())
    } else {
        MAX_CODEBASE_MEMORY_OUTPUT_BYTES
    };
    let bounded = bound_text(result, max_result_bytes);
    let anchored = correlation.is_some() && !bounded.truncated && !bounded.text.trim().is_empty();
    let text = if anchored {
        format!("{}{DECISION_ANCHOR}", bounded.text)
    } else {
        bounded.text.clone()
    };
    PresentedResult {
        text,
        provider_text: bounded.text,
        truncated: bounded.truncated,
        decision_anchor: anchored,
    }
}

/// Returns only bounded fingerprints of strings the model can select from a
/// provider result. Parsing and tokenization are transient; raw provider data
/// never enters tool details, diagnostics, or the per-run policy state.
pub(super) fn decision_anchor_evidence(result: &str) -> DecisionAnchorEvidenceV1 {
    let mut values = Vec::new();
    if let Ok(value) = serde_json::from_str(result) {
        collect_json_strings(&value, &mut values);
        let scalar_values = values.clone();
        for value in scalar_values {
            collect_text_tokens(&value, &mut values);
        }
    } else {
        collect_text_tokens(result, &mut values);
    }
    let mut seen = BTreeSet::new();
    let digests = values.into_iter().filter_map(|value| {
        GraphCorrelationV1::target_digest(&value).filter(|digest| seen.insert(digest.clone()))
    });
    DecisionAnchorEvidenceV1::new(digests)
}

fn collect_json_strings(value: &serde_json::Value, values: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value) => values.push(value.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_strings(item, values);
            }
        }
        serde_json::Value::Object(items) => {
            for item in items.values() {
                collect_json_strings(item, values);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn collect_text_tokens(result: &str, values: &mut Vec<String>) {
    for token in result.split(|character: char| {
        !character.is_ascii_alphanumeric() && !matches!(character, '_' | ':' | '/' | '.' | '-')
    }) {
        if token.len() >= 3 {
            values.push(token.to_string());
        }
    }
}

struct BoundedText {
    text: String,
    truncated: bool,
}

fn bound_text(input: &str, max_bytes: usize) -> BoundedText {
    if input.len() <= max_bytes {
        return BoundedText {
            text: input.to_string(),
            truncated: false,
        };
    }

    let notice = format!("\n[codebase-memory output truncated to {max_bytes} bytes]");
    let content_budget = max_bytes.saturating_sub(notice.len());
    let mut end = content_budget.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut text = input[..end].to_string();
    text.push_str(&notice);
    BoundedText {
        text,
        truncated: true,
    }
}
