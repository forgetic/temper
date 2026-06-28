# Verify implementation PR handoff end to end

Use this recipe to verify the implementation-PR handoff with no live Forgejo or
real LLM credentials. The hermetic real-stack test covers the product-diff path
through a real daemon, worker, native agent, local git remotes, and a Jig fake
LLM over in-process transport.

## Run the focused verification

From a fresh checkout of the revision you want to verify:

```sh
cargo test -p temper-testing --test hermetic_real_stack \
  hermetic_real_stack_basic_delivery_architect_triages_then_engineer_opens_pr -- --nocapture
```

No provider environment variables, Forgejo binaries, or host-mode Actions runner
are required.

## What the test proves

The scripted fake architect triages an intake item into a ready code spec. The
scripted fake engineer writes a product file, returns a final success summary,
and the worker pushes the branch outcome for the daemon to open an implementation
PR.

The assertions verify that the PR points at the pushed branch, contains workflow
metadata and the final summary, links back to the source issue through metadata,
and contains no model-authored implementation-plan checklist.

For live Forgejo fixture internals and the remaining ignored e2e lanes, see
[Run the daemon end-to-end fixture](run-daemon-e2e.md) and
[Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
