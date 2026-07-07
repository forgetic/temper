# Verify implementation PR handoff end to end

Use this recipe to verify the implementation-PR handoff with no live Forgejo or
real LLM credentials. The checked-in scenario drives a scripted
engineer/coding-workspace success through Temper's ForgeApplier path over
`MemoryForge` and verifies both newly-created and refreshed implementation PR
handoffs.

## Run the focused verification

From a fresh checkout of the revision you want to verify:

```sh
cargo run -p temper-scenario-cli -- check scenarios/implementation-pr-handoff
cargo run -p temper-scenario-cli -- run --tier hermetic scenarios/implementation-pr-handoff
```

The run prints `source: checked-in scenario`, `confidence tier: hermetic`, and
the manifest topology before the verdict. Asking for `--tier live` fails for
this MemoryForge-specific scenario rather than converting the hermetic proof
into a live claim.

No provider environment variables, Forgejo binaries, host-mode Actions runner,
or model credentials are required.

## What the scenario proves

The scripted workspace result supplies authored implementation PR titles and
report bodies. The assertions verify that Temper opens a new implementation PR
with the authored create handoff, refreshes an existing implementation PR with
the authored update handoff without duplicating it, clears stale body text, and
preserves workflow metadata linking each PR back to its source issue.

The live manifest `basic-delivery` scenario remains useful when you need the
broader real Forgejo + real forgejo-runner CI + real Temper + Jig fake LLM
validation-grade path, but this scenario command is the focused handoff proof.

For live Forgejo fixture internals and the remaining ignored e2e lanes, see
[Run the daemon end-to-end fixture](run-daemon-e2e.md) and
[Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
