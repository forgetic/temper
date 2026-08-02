use delivery_policy::parse_policy;

#[test]
fn parses_required_labels() {
    assert_eq!(parse_policy(" Ready, REVIEW ").required, ["ready", "review"]);
}
