# End a development session cleanly

Use this checklist before handing work to a human or another agent.

## 1. Inspect the change set

```sh
git status --short
git diff --stat
```

Use `git diff` for every file you edited. Confirm there are no generated files,
secrets, copied auth files, or unrelated Temper changes.

## 2. Run validation

For docs-only changes, formatting is usually enough. For code changes, run:

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
```

Run focused tests for behavior changes. Use `cargo dev-test` for the full
hermetic workspace when practical.

## 3. Review documentation from the top

Start with the files a fresh agent will read first:

1. `README.md`
2. `AGENTS.md`
3. `docs/README.md`

Then review task-relevant how-to, reference, explanation, and ADR files.
