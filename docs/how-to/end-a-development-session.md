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

Before pushing or opening an implementation PR, run the repo-local pre-PR script
from the repository root:

```sh
./.temper/pre-pr
```

The script runs these commands in order and stops on the first failure:

1. `cargo fmt --all -- --check`
2. `cargo depgraph-check check`
3. `scripts/check-rust-file-size.sh`
4. `scripts/check-no-ambient-env.sh`
5. Exercise the cached custom-harness permission repair against 0644 fixtures
6. `cargo dev-test-build`
7. Build nextest's exact quick-test binary set, repair custom-harness execute
   bits, then enumerate and run the captured build without invoking Cargo again
8. Drop linked test binaries from `target/debug` before linting
9. `cargo dev-clippy`

Use `cargo dev-scenario-check` plus the sole manual live-run alias,
`cargo dev-scenario-run scenarios/<name>`, when your change touches scenario
manifests, scenario runners, Forgejo/CI convergence, or validation evidence. For
an aggregate feature head, use the mapped `cargo dev-scenario-validate-feature`
command documented in
[Run focused feature validation](run-focused-feature-validation.md).
Use narrower commands only for intermediate local iteration; the cheap pre-PR
script is the required local handoff check for implementation PRs. Keep Clippy
output clean.

Keep the whole quick suite fast; as a soft target for agent changes, it should
complete in under about 10 seconds on a warmed local checkout. If a change makes
the quick suite slower, prefer moving slow coverage behind `#[ignore]` and
document how to run it before handoff.

Keep the whole full capstone suite fast too; as a soft target for agent
changes, it should complete in under about 2 minutes on a warmed local checkout.
The exhaustive `cargo dev-test-e2e-all` lane is owned by CI and may also be run
manually when a change touches live e2e behavior.

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
scripts/check-rust-file-size.sh
```

## 5. Leave an explicit handoff

In the final response, commit message or Forge comment, include:

- what changed
- validation commands run
- important limitations or follow-up work
- files the next agent should read first, if not obvious
