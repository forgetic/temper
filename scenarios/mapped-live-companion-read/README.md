# Mapped live companion read

This scenario maps feature `ai/temper#1304` on
`agent/pr-for-feature-1304`. It reuses the historical graph-consumption provider,
seed, workflow, CI, and graph counts without changing the #1009 mapping.

After the six typed graph calls, local graph denial, and ordinary primary read,
the live Jig requests one patch editing both the primary and the existing
README. That patch must be denied locally because the README is unread. Jig
then reads the README and retries the identical patch. Host validation, real
Actions CI, PR merge, and issue closure must succeed.

The host verifies that the README existed with the expected seed bytes, fetches
the merged default branch, requires its SHA to equal the recorded merge SHA,
and compares the committed README with the expected changed bytes. A required
assertion reads the closed `existing-seed-file-matches-merged-change` checkpoint.
Working-tree content alone cannot satisfy the check.

Run `cargo dev-scenario-run scenarios/mapped-live-companion-read` from the
validated source head, or use the documented focused validation command for
`ai/temper#1304` with the landing branch, PR, SHA, and output directory.
This is a functional Jig delivery proof, not real-model timing evidence.

The inherited seed has no `Cargo.lock`; its validation command generates one.
The delivery diff therefore contains the two patched files and that lockfile.
