# Verify checkpoint implementation PRs end to end

Use this recipe to verify the checkpoint-to-implementation-PR handoff with no live Forgejo or real LLM credentials. The former root `temper run` live e2e for this story was removed because the hermetic real-stack test covers the same product-diff handoff through a real daemon, worker, native agent, local git remotes, and a Jig fake LLM over in-process transport.

## Run the focused verification

From a fresh checkout of the revision you want to verify:

```sh
cargo test -p temper-testing --test hermetic_real_stack \
  hermetic_real_stack_checkpointed_product_diff_finalizes_implementation_pr -- --nocapture
```

No provider environment variables, Forgejo binaries, or host-mode Actions runner are required.

## What the test proves

The scripted fake engineer performs the checkpoint-only flow:

1. Writes `DELIVERY.md` into the prepared checkout.
2. Calls `checkpoint` after the product edit exists, opening an implementation PR from the checkpointed branch.
3. Returns the final success summary, which finalizes the same PR body.

The assertions verify that the final PR uses the checkpoint commit as its head, contains workflow metadata and the final summary, links back to the source issue, and does not contain a model-authored implementation-plan checklist.

For live Forgejo fixture internals and the remaining ignored e2e lanes, see [Run the daemon end-to-end fixture](run-daemon-e2e.md) and [Forgejo e2e fixture reference](../reference/forgejo-e2e-fixture.md).
