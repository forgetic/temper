# Verify implementation PR handoff end to end

Use this recipe to verify the implementation-PR handoff on Temper's live
manifest scenario stack: real Forgejo, real host `forgejo-runner` Actions CI,
real standalone Temper, and Jig fake LLM responses.

## Run the live scenario

From a fresh checkout of the revision you want to verify:

```sh
cargo build --bin temper
cargo run -p temper-scenario-cli -- check scenarios/implementation-pr-handoff
cargo run -p temper-scenario-cli -- run --tier live \
  --temper-bin target/debug/temper \
  scenarios/implementation-pr-handoff
```

The run selects `runner: manifest`, captures JSON Temper observability, and
evaluates the declarative `[[expect.checks]]`, `[[expect.events]]`,
`[[expect.sequence]]`, and `[[expect.count]]` entries in `scenario.toml`.

## What the scenario proves

The scripted engineer workspace result supplies authored implementation PR titles
and report bodies. The assertions verify that Temper opens a new implementation
PR with the authored create handoff, refreshes an existing stale implementation
PR without duplicating it, clears stale body text, and preserves workflow
metadata linking each PR back to its source issue and correlation key.

The default hermetic tier is intentionally rejected; this scenario no longer has
a MemoryForge or in-process runner substitute.
