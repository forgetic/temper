# Migration evidence and remaining work

This is the migration's checkpoint ledger. The requirements remain the
steps in README.md and their own documents; an entry here does not replace
their completion criteria. Branches are local. Nothing has been pushed.

## Groundwork

| Increment | State | Evidence |
|---|---|---|
| 00a Forgejo facts | merged, `fa97784` | isolated Forgejo 15 probe and retained observations; all four workflow checks passed, focused 1,772/1,772 in 7.440 s, fuzzy 26/26 in 22.148 s, one ignored finding |
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

Step 00 is complete. All seven Forgejo prerequisites have observations
or the specified conservative fallback in `domain/forge.md`, 20.1. The
retained fixture contains 53 actual HTTP exchanges and two signed
webhooks; both HMACs were independently verified. No scratch server or
runner remains live. Comparison paging is unsupported, protection reads
are denied to write permission, and supported REST job-log reads are
unavailable; the design records bounded/unknown handling and status links.

| Increment | State | Evidence |
|---|---|---|
| 01a authority values and order | merged, `2d38f2b` | all four workflow checks passed; focused 1,778/1,778 in 7.918 s, fuzzy 26/26 in 22.367 s, one ignored finding; authority serial total 0.087 s |
| 05a fleet turns and graces | merged, `c6513cb` | all four workflow checks passed; focused 1,794/1,794 in 7.478 s, fuzzy 27/27 in 22.515 s, one ignored finding; fleet focused serial total 0.144 s and fuzzy 0.508 s |

01a checks pattern inclusion against independently enumerated names and
authority ordering laws. 05a adds bounded turn admission, commitment
acknowledgements, prefix restoration with adoption, fencing and busy
retry; its separate turn world checks reconnects, restart, ownership,
replay, facts and declared stop bounds. Existing fleet scenarios remain.
The legacy root supplies the first version's defaults and exhaustive
ignore arms for additions.

Steps 01 and 05 are partially implemented. Authority funding arithmetic
(01b), people sign-in and initial requests (03a), and channel version two
(05b) are in isolated worktrees. Steps 02, 04 and 06 through 08 remain.

After groundwork, the plan's finer dependencies still apply: tasks needs
authority's value and number shapes; the root's walking skeleton needs
01a–c, 02a–b, 03a and 05a. Runtime payload shapes settle in 05b before
dependent extensions. The legacy engine and its worlds stay green and
frozen until the single cutover.

Step 08 includes domain notes and contractions, followed by writing the
separate plans for store, web, credentials, forge protocol and the engine's
iteration/shell. Implementing those lower layers is later work, as
08-after.md, section 3, specifies.
