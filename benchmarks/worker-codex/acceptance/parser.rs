use delivery_policy::{
    ParseErrorKind as Kind, ParsePolicyError, PolicyOptions, Severity, parse_rule_policy,
};

fn assert_error(input: &str, line: usize, kind: Kind, message: &str) {
    let error = parse_rule_policy(input, PolicyOptions::default()).unwrap_err();
    assert_eq!(error, ParsePolicyError { line, kind });
    assert_eq!(error.to_string(), format!("{message} at line {line}"));
    let _: &dyn std::error::Error = &error;
}

#[test]
fn parses_comments_crlf_case_colons_and_literal_hashes() {
    let options = PolicyOptions {
        promote_advisory: true,
        label_prefix: Some(" Tag: ".into()),
    };
    let policy = parse_rule_policy(
        " \r\n # ignored\r\n REQUIRED : Ready \r\nadvisory: TEAM:Core\r\nAdViSoRy: Docs#Public\r\nrequired:ÄBC\r\n",
        options.clone(),
    ).unwrap();
    assert_eq!(policy.options, options);
    assert_eq!(
        policy
            .rules
            .iter()
            .map(|rule| rule.label.as_str())
            .collect::<Vec<_>>(),
        ["ready", "team:core", "docs#public", "Äbc"]
    );
    assert_eq!(
        policy
            .rules
            .iter()
            .map(|rule| rule.severity)
            .collect::<Vec<_>>(),
        [
            Severity::Required,
            Severity::Advisory,
            Severity::Advisory,
            Severity::Required
        ]
    );
}

#[test]
fn default_options_and_empty_policy() {
    let options = PolicyOptions::default();
    assert!(!options.promote_advisory);
    assert_eq!(options.label_prefix, None);
    for input in ["", " \n\t", "# comment\n\n  # another"] {
        assert!(
            parse_rule_policy(input, options.clone())
                .unwrap()
                .rules
                .is_empty()
        );
    }
}

#[test]
fn typed_errors_preserve_physical_lines_and_precedence() {
    assert_error(
        "# heading\n\nrequired ready",
        3,
        Kind::MissingSeparator,
        "missing ':'",
    );
    assert_error("unknown", 1, Kind::MissingSeparator, "missing ':'");
    assert_error(
        "optional:ready",
        1,
        Kind::UnknownSeverity,
        "unknown severity",
    );
    assert_error("optional:  ", 1, Kind::UnknownSeverity, "unknown severity");
    assert_error(":ready", 1, Kind::UnknownSeverity, "unknown severity");
    assert_error("required:\t", 1, Kind::EmptyLabel, "empty label");
    assert_error(
        "advisory:docs\r\n# note\r\nrequired: DOCS ",
        3,
        Kind::DuplicateLabel,
        "duplicate label",
    );
    assert_error(
        "required:ready\nrequired:READY\nunknown:x",
        2,
        Kind::DuplicateLabel,
        "duplicate label",
    );
    assert_error(
        "required:\nrequired:x\nrequired:X",
        1,
        Kind::EmptyLabel,
        "empty label",
    );
}

#[test]
fn rule_duplicates_are_independent_of_observation_prefix() {
    let policy = parse_rule_policy(
        "required:ready\nadvisory:tag:ready",
        PolicyOptions {
            promote_advisory: false,
            label_prefix: Some("tag:".into()),
        },
    )
    .unwrap();
    assert_eq!(policy.rules.len(), 2);
    assert_eq!(policy.rules[1].label, "tag:ready");
}

#[test]
fn parser_preserves_non_ascii_case() {
    let policy = parse_rule_policy("required:ÄBC\nadvisory:äbc", PolicyOptions::default()).unwrap();
    assert_eq!(policy.rules[0].label, "Äbc");
    assert_eq!(policy.rules[1].label, "äbc");
}
