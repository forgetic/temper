# End a development session cleanly

Use this checklist before handing work to a human or another agent.

## 1. Inspect the change set

```sh
git status --short
git diff --stat
```

Use `git diff` for any file you edited. Confirm there are no accidental
generated files, secrets, or unrelated changes.

## 2. Run validation

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
cargo dev-test-quick
```

Clippy is installed in this environment; keep its output clean. The quick test
suite should remain fast (soft target: under about 10 seconds on a warmed local
checkout).

Then run the full self-contained local suite before handoff:

```sh
cargo dev-test-full
```

It must be green; fix failures instead of handing them off. This runs the quick
suite plus all ignored local-process/local-Forgejo tests. On a networked
machine, first startup downloads pinned Forgejo binaries automatically when
`.cache/forgejo/` is empty.

Default libtest parallelism is supported. Use
`cargo dev-test-full --test-threads=1` only as an optional host resource
throttle when the machine cannot comfortably run several real Forgejo/runner
processes at once.

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

Update docs in the same session as the code change. Do not leave stale status
notes, obsolete commands, or undocumented behavior changes. If a session reveals
durable steering, fold it into the relevant canonical doc, test, or ADR during
this review.

## 4. Check file size budgets

Keep hand-written documents focused. Aim for about 150 lines or fewer and split
before about 350 lines. If a file is getting large, create a short index and
move details into smaller pages.

Keep Rust source and test files at or below 600 lines. If a file is getting
larger, split it into focused modules or shared test support.

Useful checks:

```sh
wc -l README.md AGENTS.md docs/**/*.md
find crates -type f -name '*.rs' -print0 | xargs -0 wc -l | sort -n
```

## 5. Leave an explicit handoff

In the final response or commit message, include:

- what changed
- validation commands run
- important limitations or follow-up work
- files the next agent should read first, if not obvious
