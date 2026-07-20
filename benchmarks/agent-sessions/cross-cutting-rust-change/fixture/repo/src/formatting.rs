/// Produces the canonical storage form for a label.
pub fn normalize_label(input: &str) -> String {
    input.trim().to_ascii_lowercase()
}
