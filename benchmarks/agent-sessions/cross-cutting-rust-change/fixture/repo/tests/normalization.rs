use label_normalizer::normalize_label;

#[test]
fn normalizes_whitespace_and_case() {
    assert_eq!(normalize_label("  Ready FOR Review "), "ready for review");
}
