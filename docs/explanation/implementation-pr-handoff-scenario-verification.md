# Implementation PR handoff scenario verification report

Target feature: #52 / PR #53, "Engineer PR handoff: carry agent-authored implementation PR title/body through PR create/update". The checked-in scenario now runs through the generic manifest runner on the live validation stack.

## Verdict

**Pass on the implicit live topology.**
`scenarios/implementation-pr-handoff` declares `runner.uses = "manifest"` and
uses real Forgejo, a host `forgejo-runner` for Actions CI readiness, a
standalone Temper process, and Jig fake-LLM engineer responses. MemoryForge,
filesystem-forge, in-process, hermetic real-stack, and simulation tests remain
lower-level coverage and are not evidence from this landing scenario.

## Behavior contract

The scenario validates that:

- a no-verdict engineer/coding-workspace success may author `title` and `body` for an implementation PR handoff;
- PR creation uses the authored title/body instead of generic fallbacks;
- refreshing an existing implementation PR replaces stale generated handoff text without opening a duplicate PR;
- the PR body remains classified as `metadata.kind = "implementation_pr"` and retains the source issue parent and `pr-for-code-<issue>` correlation key;
- Temper emits structured `pr.opened` / `pr.updated` events after the Forge operation succeeds, including `title.source`, `body.source`, `metadata.*`, `source_artifact`, `correlation.key`, and `action = "created" | "refreshed"` facts.

## Commands

```sh
cargo run -p temper-scenario-cli -- check scenarios/implementation-pr-handoff
cargo dev-scenario-run scenarios/implementation-pr-handoff
```

`cargo dev-scenario-run scenarios/<name>` is the sole manual live-run alias. It
builds standalone Temper and invokes the scenario through the implicit manifest
topology. Expected evidence includes the manifest runner identity, topology/log
paths, create and refresh PR evidence lines, and passing manifest assertions
over final PR state plus structured observability events.

Latest workspace validation for this port ran:

```sh
target/debug/temper-scenario run \
  --temper-bin target/debug/temper \
  scenarios/implementation-pr-handoff
```

The run reported runner id `manifest`, passed on the implicit live stack, and reported `assertions: passed (10 passed, 0 failed, 0 unsupported)` including `expect.created_pull_requests`, `expect.refreshed_pull_requests`, the `pr.opened`/`pr.updated` handoff events, and the create-then-refresh sequence.

## Implementation notes

- `scenario.toml` declares live setup primitives for Forgejo, `forgejo-runner`, repo seeding, Jig fake LLM scripts, standalone Temper launch, source issue seeding, stale PR seeding, convergence waiting, and event/final-state assertions.
- `temper-log` now includes `pr.updated` and enriched handoff fields on `pr.opened` / `pr.updated`.
- The manifest live harness drives the real Temper workflow path; it coordinates stale PR seeding only after the refresh engineer job has been claimed so the refresh exercises the result-applier update path rather than a MemoryForge helper.
