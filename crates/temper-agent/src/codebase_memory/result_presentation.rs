use super::MAX_CODEBASE_MEMORY_OUTPUT_BYTES;

/// Neutral marker for one complete typed wrapper result. The pure machine
/// appends the authoritative active-root classification after settlement.
pub(super) const DECISION_ANCHOR: &str = "\n\n[Decision anchor: complete typed graph result; active-root eligibility is pending the stateful Decision guidance appended after this result. Do not choose a dependent graph call until that guidance classifies this result.]";
pub(super) const RECOVERY_SELECTOR_GUIDANCE_RESERVE_BYTES: usize = 1536;

pub(super) struct PresentedResult {
    pub(super) text: String,
    pub(super) truncated: bool,
    pub(super) decision_anchor: bool,
}

/// Keeps the complete model-visible result, including its optional anchor,
/// within the existing output limit. A result that needs truncation remains
/// useful discovery, but is intentionally not a decision anchor.
pub(super) fn present_result(
    result: &str,
    correlation: Option<&temper_protocol_activity::GraphCorrelationV1>,
) -> PresentedResult {
    let max_result_bytes = if correlation.is_some() {
        MAX_CODEBASE_MEMORY_OUTPUT_BYTES
            .saturating_sub(DECISION_ANCHOR.len())
            .saturating_sub(RECOVERY_SELECTOR_GUIDANCE_RESERVE_BYTES)
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
        truncated: bounded.truncated,
        decision_anchor: anchored,
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
