# End a development session cleanly

Use this checklist before handing work to a human or another agent.

## 1. Inspect the change set

```sh
git status --short
git diff --stat
```

Use `git diff` for any file you edited. Confirm there are no accidental generated files, secrets, or unrelated changes.

## 2. Run the fast validation loop

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
```

Clippy is installed in this environment; keep its output clean. Add task-specific tests when behavior changed. Run those tests before handoff.

## 3. Review documentation from the top

Start with the files a fresh agent will read first:

1. `README.md`
2. `AGENTS.md`
3. `docs/README.md`

Then review task-relevant docs:

- reference docs for API or behavior changes
- explanation docs for conceptual changes
- how-to guides for workflow changes
- ADRs for significant decisions

Update docs in the same session as the code change. Do not leave stale status notes, obsolete commands, or undocumented behavior changes.

## 4. Capture lessons learned

If the session involved a human correction, failed assumption, repeated mistake, or missing guidance, add or update an entry in `docs/reference/agent-lessons/`.

Use `docs/how-to/record-agent-lesson.md`.

## 5. Check file size budgets

Keep hand-written documents focused. Aim for about 150 lines or fewer and split before about 350 lines. If a file is getting large, create a short index and move details into smaller pages.

Keep Rust source and test files at or below 600 lines. If a file is getting larger, split it into focused modules or shared test support.

Useful checks:

```sh
wc -l README.md AGENTS.md docs/**/*.md
find crates -type f -name '*.rs' -print0 | xargs -0 wc -l | sort -n
```

## 6. Leave an explicit handoff

In the final response or commit message, include:

- what changed
- validation commands run
- important limitations or follow-up work
- files the next agent should read first, if not obvious
