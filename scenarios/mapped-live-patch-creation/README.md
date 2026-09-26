# Mapped live patch creation

This scenario maps feature `ai/temper#1303` on
`agent/pr-for-feature-1303`. It inherits the historical
`mapped-live-graph-consumption` seed, provider, workflow, CI, and graph-count
assertions. The historical #1009 mapping remains unchanged.

The runtime reuses its six typed graph calls, local convergence denial, and
ordinary primary read. It then applies one patch that repairs `src/lib.rs` and
creates the previously absent `tests/created_dispatch.rs`. Existing tests and
the new regression run through host validation and real Actions CI. The live
harness fetches the default branch after merge, checks that its SHA equals the
recorded merged SHA, and compares the created Git blob with the expected bytes.
A required assertion verifies the resulting closed checkpoint in the retained
repository log. Workspace bytes alone cannot satisfy this proof.

Run `cargo dev-scenario-run scenarios/mapped-live-patch-creation` from the
validated source head, or resolve this mapping with
`cargo dev-scenario-validate-feature --feature ai/temper#1303` and the documented
landing branch, PR, SHA, and output arguments. This Jig delivery proof supplies
functional evidence; it does not measure real-model performance.

Provider values and source contents remain in temporary runtime state. Retained
aggregate evidence contains only the existing graph facts and the closed
`absent-seed-file-matches-merged-bytes` checkpoint.

The inherited seed has no `Cargo.lock`; its validation command generates one.
The delivery diff therefore contains the two patched files and that lockfile.
