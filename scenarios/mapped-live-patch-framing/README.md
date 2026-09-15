# Mapped live patch framing

This scenario maps `ai/temper#1305` to the implicit live topology: real Forgejo,
a host runner, standalone Temper, and a Jig model. It extends the verified
creation fixture and keeps its six typed graph calls, local graph closure,
primary implementation read, host validation, Actions CI, merge, and issue closure.

The single cohesive patch deliberately has three recoverable envelope defects:
wrong line counts in both hunks, an indented second `diff --git` header, and no
`new file mode` for its explicit `/dev/null` creation. Its actual context and
added/deleted lines are unchanged from the working creation fixture. Git alone
rejects this input; worker admission and execution must share its normalization.

After merge, the host checks the exact generated regression bytes and exact
changed paths: `Cargo.lock`, `src/lib.rs`, and `tests/created_dispatch.rs`.
The inherited seed has no lockfile, so its validation command creates one.
The historical creation and companion scenarios retain their own mappings.

Run `cargo dev-scenario-run scenarios/mapped-live-patch-framing` from the
repository root. The focused feature alias resolves this scenario for
`agent/pr-for-feature-1305` and records the exact source head.
