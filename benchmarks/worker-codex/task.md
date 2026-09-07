# Add configurable delivery rules without breaking required-label callers

The `delivery-policy` crate parses required labels, evaluates observations, and
renders decisions. Add a configurable rule API across its model, parser,
evaluator, reporting, and public exports. Keep it dependency-free. Preserve
the existing `Policy { required }` and `Decision { accepted, missing }` public
struct literals and all existing functions and behavior, including exact
matching of observations in the legacy `evaluate_labels` function.

Expose these additional items from the crate root. All named fields below are
public. All types derive `Clone`, `Debug`, `Eq`, and `PartialEq`; `Severity` and
`ParseErrorKind` also derive `Copy`.

```rust
enum Severity { Advisory, Required }
struct PolicyOptions {
    promote_advisory: bool,
    label_prefix: Option<String>,
}
// Default: promote_advisory = false, label_prefix = None.
struct Rule { severity: Severity, label: String }
struct RulePolicy { rules: Vec<Rule>, options: PolicyOptions }
enum ParseErrorKind {
    MissingSeparator, UnknownSeverity, EmptyLabel, DuplicateLabel,
}
struct ParsePolicyError { line: usize, kind: ParseErrorKind }
struct Violation { severity: Severity, label: String }
struct Evaluation {
    accepted: bool,
    required: Vec<Violation>,
    advisory: Vec<Violation>,
}
struct Summary { required: usize, advisory: usize, total: usize }

fn parse_rule_policy(input: &str, options: PolicyOptions)
    -> Result<RulePolicy, ParsePolicyError>;
fn evaluate_rule_policy(policy: &RulePolicy, labels: &[&str]) -> Evaluation;
fn summarize(evaluation: &Evaluation) -> Summary;
fn render_rule_decision(evaluation: &Evaluation) -> String;
```

Parsing rules:

- Read one `severity:label` per line. Ignore blank lines and lines whose first
  non-whitespace character is `#`. Inline `#` characters are ordinary label
  content. Support LF and CRLF; error line numbers include comments/blanks.
- Split at the first colon. Normalize severity and label by trimming whitespace
  and applying ASCII lowercase. Accept only `required` and `advisory` severity.
  Labels may contain further colons and non-ASCII characters; non-ASCII case is
  preserved. Reject labels that normalize to the empty string.
- Reject duplicate normalized labels, even when their severities differ. Keep
  declaration order. Return the first erroneous line. On that line check missing
  separator, then unknown severity, then empty label, then duplicate label.
- Store the supplied options unchanged. An empty policy is valid.
- Implement `std::error::Error` and `Display` for `ParsePolicyError`. The exact
  display is `<message> at line <line>`, with these messages: `missing ':'`,
  `unknown severity`, `empty label`, and `duplicate label`, respectively.

Evaluation rules:

- For the new API only, normalize each observed label with trim + ASCII
  lowercase. If `label_prefix` is `Some`, normalize that prefix the same way,
  strip it once from the start of each normalized observation when present,
  then trim the result. An empty normalized prefix has no effect. Observations
  without the prefix remain eligible. Never strip the prefix from rule labels.
- Match normalized observations against rule labels exactly. Ignore surplus
  and duplicate observations. Do not mutate the policy. `RulePolicy` values
  constructed directly by callers have already normalized rule labels; callers
  may include duplicate rules, and each missing rule produces a violation.
- Produce one `Violation` per missing rule, preserving declaration order within
  each severity group and copying its label/severity. Required violations make
  the result rejected. Advisory violations make it rejected only when
  `promote_advisory` is true; promotion never moves them to the required group.
- `summarize` counts the required and advisory violation vectors; `total` is
  their sum. `render_rule_decision` uses `evaluation.accepted` and emits exactly
  four lines with no trailing newline. Join labels in each group with `, `;
  use `-` for an empty group. For example:

```text
decision: rejected
required: review
advisory: docs, changelog
counts: required=1, advisory=2, total=3
```

For an accepted result the first line is `decision: accepted`. Counts must
reflect the supplied vectors even for a directly constructed `Evaluation`.

Add focused parser/evaluator/report tests and an integration workflow example,
document the new API and legacy compatibility in README, and update CHANGELOG.
Run `cargo fmt --all -- --check` and `cargo test --offline --quiet`, then inspect
the full diff. Follow AGENTS.md for discovery and session completion.
