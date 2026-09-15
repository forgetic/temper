# Mapped live batched edits

This scenario maps `ai/temper#1318`. It uses the inherited real Forgejo,
host runner, standalone Temper, and six-call graph fixture.

After reading the selected primary, the model submits one `edit_files` call
for that file and its unread existing README. The whole batch must be denied.
The model reads the README and retries the identical two-file batch; the
unchanged old text in that retry also detects a premature primary write.
It validates, submits, passes CI, and merges. The host reads both exact blobs
and the complete changed-path set from the actual merged default-branch SHA.

Run from the repository root:

```sh
cargo dev-scenario-run scenarios/mapped-live-batched-edits
```
