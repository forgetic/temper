# Bounded Forgejo Actions pagination and automatic landing

This active mapped-live scenario validates feature `ai/temper#1055`, plan
`ai/temper#1057`, and inherited feature branch
`agent/pr-for-feature-1055`. It inherits only the stable basic-delivery Forgejo,
workflow, repository, and CI shape. It owns a local Jig script and local copies
of the workflow, repository, and CI fixtures used by its pagination contract.

## Claim, stimulus, observable, assertion

- **Claim:** Every Forgejo Actions inventory read remains explicitly paged and
  an exact-head-green run remains discoverable after page one, allowing the
  dedicated CI-success path to land automatically.
- **Stimulus:** Disposable Forgejo 16.0.1, the host `forgejo-runner`, standalone
  Temper, and Jig agents create exactly one implementation PR and one real
  Actions run. After provider jobs materialize, the harness inserts 201 bounded
  90,000-byte historical rows ahead of that run, disables the disposable
  repository's bounded webhook inventory, and measures the now-oversized
  inventory while both the real workflow and standalone Temper remain active.
  The Jig never merges or repairs the PR.
- **Observable:** Structured evidence retains the transport cap, bounded row and
  byte counts, largest page, page count, selected target page, request-key names,
  exact head/run/job identities, effective cadences, automation merger identity,
  and ordered `ci.completed`, `pr.merged`, and `item.resolved` events.
- **Assertion:** The full-inventory lower bound exceeds 16 MiB while every page
  remains below it; multiple pages are observed; the selected run is after page
  one; no request provenance is dropped; all `/actions/runs` reads carry both
  `page` and `limit`; no legacy, UI, login, tasks, live-view, or mutation route is
  used; a fresh targeted success/landing sequence is paired with the structured
  `trigger.source=ci_poll` observation, while both broad fallbacks remain
  configured at 600 seconds.

## Causality and terminal state

The oversized-history action runs only after the one implementation PR and its
real exact-head provider jobs have materialized. A bounded CI hold keeps that
run in flight while the action inserts synthetic rows and disables the bounded
repository-hook inventory, so the target is moved beyond page one without
introducing a second implementation head or stopping standalone Temper. This
isolates webhook-less CI detection after all PR publication effects are settled.
Forgejo 16.0.1 supplies bounded pages to the production adapter. The run then
completes green, the dedicated one-second CI poll submits an exact CI wake, and
its fresh targeted scan automatically lands the PR. The structured `ci_poll`
observation confirms that path after fresh execution; the broad role-feed poll
and periodic mechanical reconciliation remain delayed for 600 seconds.

The manifest requires one PR publication, a green exact-head CI observation,
the targeted success-and-landing sequence plus dedicated source event, a closed
parent, and a merger from the harness's closed automation identity set
(`basicadmin` or `bot`), never the engineer. The scenario contains no manual
merge, Actions mutation, repair, or broad-fallback stimulus.

## Privacy boundary

Checked-in fixtures and run evidence contain no credentials or generated
runtime evidence. Retained request provenance is limited to method, normalized
path, query-key names, authentication scheme/presence, and JSON acceptance.
Oversized-history evidence contains only bounded counts, sizes, page numbers,
booleans (including webhook isolation), identities, and timing facts. Response
bodies, synthetic provider rows, event payloads, authorization values, tokens,
unrelated headers, database copies, logs, and temporary Forgejo state are never
retained in the scenario corpus.

## Running

From the exact assembled feature head:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/forgejo-actions-pagination-landing
cargo dev-scenario-validate-feature \
  --feature ai/temper#1055 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1055 \
  --pr <scenario-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation-1055
./.temper/pre-pr
```

Generated logs and evidence belong only in the caller-owned output directory.
