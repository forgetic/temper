# Mapped live schema rejection feedback

This scenario maps `ai/temper#1322` and inherits the real Forgejo, host runner,
standalone Temper, and six-call graph fixture from the batched-edit scenario.

After the primary read, the model calls `edit_files` with a missing `oldText`
field and a sentinel unknown key and replacement value. The next model request
must identify `edit_files` and `$.files[].edits[].oldText`, without either
sentinel anywhere in the recorded request body. It then reuses the existing
well-shaped two-file batch: denial while the companion is unread, an exact
companion read, and an identical successful retry. That retry's original text
also detects premature writes. Both exact merged blobs and the complete
three-path diff are checked against the actual merged commit.

```sh
cargo dev-scenario-run scenarios/mapped-live-schema-rejection-feedback
```
