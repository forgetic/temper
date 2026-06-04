# Findings — bounded reconciliation and correlation lookups

## Before

- Normal mechanical reconciliation could load `IssueQuery::default()` and
  `PullRequestQuery::default()`, which map to all-state full-detail lists and
  include closed or merged unlabelled history.
- Incomplete journal recovery depended on broad scans to see targets that had
  lost workflow labels.
- Idempotent issue/PR create retries found a correlation key by listing all
  states with full detail before parsing metadata.

## After

- Normal reconciliation first exact-reads incomplete journal targets, then lists
  workflow-labelled candidates with explicit states and summary detail. Exact
  full-detail reloads are limited to dependency-gated artifact kinds.
- Runner mechanical ticks use bounded reconciliation; `tick_deep_audit` and
  production/testing audit intervals are the explicit all-history diagnostic
  path.
- Normal create retries query explicit states with summary detail, create labels
  when available, and an escaped metadata body marker before exact metadata
  confirmation.
- Forgejo mock-contract tests lock in the hot-path request shapes: bounded
  reconciliation emits state+label issue-index requests and skips dependency
  enrichment, while labelled correlation lookup never starts with
  `/issues?state=all`, `/pulls?state=all`, or unlabelled closed history.

## Caveats

- Forgejo 7.0.x does not provide reliable exact provider-side body search.
  `body_contains` is therefore applied client-side after the narrowest available
  state/label provider query; no `q`/`body` parameter is sent.
- Compatibility discovery of legacy artifacts that do not carry the create
  labels remains outside the normal labelled create path. Operators can use the
  explicit deep-audit path for rare historical diagnostics.
