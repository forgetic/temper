# `temper plan` report

`temper plan` loads the same canonical deployment bundle as `temper apply`, but
uses only the Forge inspection capability. It never mints a token and never
creates or updates users, repositories, labels, webhooks, credentials, or local
bundle files. A resolved admin token is preferred. Before the first apply, a
bundle with only the configured admin login and password is inspected through
the mutation-proof Basic-auth adapter. If neither authentication form is
available, every configured repository is still reported with unknown state and
an unavailable inspection finding.

## Repository collection

Human output has one repository section for every `[engine] repos` entry. Forge
lookup, labels, webhook readiness, CI readiness, workflow metadata, and
`--existing-repo` findings are repository-scoped. One failed or unavailable
lookup does not prevent inspection of later repositories. Deployment status,
result, Forge inspection state, identities, and top-level findings aggregate the
whole collection.

## JSON contract and migration

The JSON report includes numeric `report_version: 1` and a `repositories` array.
Each array entry has these keys:

- `repository`
- `labels`
- `webhook`
- `metadata`
- `findings`

For a deployment with exactly one repository, Temper also emits the historical
top-level `repository`, `labels`, `webhook`, and `metadata` keys. Their value
types and values are unchanged, and each equals the corresponding value in
`repositories[0]`. Existing single-repository consumers can migrate by reading
the collection and may continue reading the compatibility projection while they
do so.

For a deployment with multiple repositories, those four singular keys are
omitted. Consumers must iterate `repositories`; treating a missing singular key
as an error is not compatible with multi-repository deployments. Top-level
`status`, `result`, `forge`, and `findings` always describe the complete
collection.

Secrets are never report values. Webhook secrets render as `<redacted>` and
Forge/provider credentials are absent from both human and JSON output.
