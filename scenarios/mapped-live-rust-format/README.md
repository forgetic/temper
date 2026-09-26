# Explicit Rust formatting after exact target reads

This maps `ai/temper#1314` to a real Forgejo, host runner, standalone Temper,
and Jig delivery. It inherits the creation seed and the historical six-call
graph transcript without changing either existing feature mapping.

The worker reads the selected implementation and applies a valid two-file
patch whose primary source and newly created regression both need formatting.
An identical `format_rust` call with both explicit paths and edition `2021`
is denied while the new regression is unread. After its ordinary exact read,
the same formatting call succeeds. Conventional formatting checks and tests,
Actions, PR merge, and issue closure must then converge.

The host verifies the new regression was absent from the seed, both Rust files
match their exact formatted Git blobs at the recorded merged default-branch
SHA, and the complete diff is exactly `Cargo.lock`, `src/lib.rs`, and
`tests/created_dispatch.rs`. The inherited validation generates the lockfile.
The primary declares `pub mod caller`; the unchanged caller file must stay out
of the delivered diff.

Run with `cargo dev-scenario-run scenarios/mapped-live-rust-format` after the
full pre-PR lane and `cargo dev-scenario-check`.
