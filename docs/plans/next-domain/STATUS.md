# Migration evidence and remaining work

This is the migration's checkpoint ledger. The requirements remain the
steps in README.md and their own documents; an entry here does not replace
their completion criteria. Branches are local. Nothing has been pushed.

## Groundwork

| Increment | State | Evidence |
|---|---|---|
| 00a Forgejo facts | probing complete; changes not yet merged | isolated Forgejo 15 probe; conformance branch `next-domain/00a` still in preparation |
| 00b dead drafts | merged, `9acb981` | all four workflow checks passed; 1,772 focused tests, 26 fuzzy tests and one ignored finding, unchanged from baseline |
| 00c legacy rename | merged, `d8385dc` | all four workflow checks passed; focused 1,772/1,772 in 7.495 s, fuzzy 26/26 in 22.159 s, one ignored finding |
| 00d design destination | merged, `be9164e` | Markdown only; design now under `docs/design/domain/` |
| 00e durable-state convention | merged, `15d5708` | Markdown only; `domain/engine.md`, 5.6 |
| 00f budgets | merged, `8f11611` | Markdown only; measured baseline and initial allotments in README.md, 5.5 |

The 00c static comparison against `9acb981` checked all 718 tracked files:
the only changes are the nine directory moves, exact package and Rust
identifier substitutions, nine legacy descriptions, five frozen root-doc
lines, lockfile ordering, and rustfmt. Runtime string literals do not
contain any renamed package or path. The unchanged focused/fuzzy suites
include deterministic replay checks. No legacy behavior was altered.

The baseline at `95e9bfd` passed formatting, clippy and both enforced
profiles. The focused profile took 5.758 s; fuzzy took 18.223 s. The two
serial measurement runs took 19.348 s and 41.076 s respectively. Each
world's serial total is recorded in README.md, 5.5. Test budgets remain
15 seconds focused and 60 seconds fuzzy.

## Remaining steps

00a must be merged and its facts reviewed before new domain code begins.
Steps 01 through 08 remain unimplemented. The authority value/order
increment (01a) has been prepared read-only; it has no code yet.

After groundwork, the plan's finer dependencies still apply: tasks needs
authority's value and number shapes; the root's walking skeleton needs
01a–c, 02a–b, 03a and 05a. Runtime payload shapes settle in 05b before
dependent extensions. The legacy engine and its worlds stay green and
frozen until the single cutover.

Step 08 includes domain notes and contractions, followed by writing the
separate plans for store, web, credentials, forge protocol and the engine's
iteration/shell. Implementing those lower layers is later work, as
08-after.md, section 3, specifies.
