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
| 01b funding arithmetic | merged, `c80d438` | all four workflow checks passed; focused 1,829/1,829 in 5.256 s, fuzzy 28/28 in 22.473 s, one ignored finding; authority serial total 0.115 s |
| 01c authority policies and checks | merged, `e21dafb` | independent review passed; all four workflow checks passed; focused 1,850/1,850 in 4.888 s, fuzzy 28/28 in 18.143 s, one ignored finding; complete authority serial total 0.131 s |
| 01d landing requirements | merged, `e9aea6d` | independent review corrected missing-facts refusal precedence; all four workflow checks passed; focused 2,118/2,118 in 4.506 s, fuzzy 30/30 in 16.345 s, one ignored finding; authority serial total 0.164 s |
| 02a tasks batches and lifecycle | merged, `376cbd4` | independent review corrected held-closing creation and exhaustive matches; recovery regression passed; all four workflow checks passed; focused 2,115/2,115 in 5.532 s, fuzzy 30/30 in 20.070 s, one ignored finding; tasks serial focused 0.117 s, fuzzy 0.781 s |
| 03a people sign-in and chat requests | merged, `959795f` | all four workflow checks passed; focused 1,822/1,822 in 7.251 s, fuzzy 28/28 in 22.695 s, one ignored finding; targeted people serial total 0.098 s before the added root-pressure regression |
| 04a1 fake forge git foundations | merged, `0d09efc` | all four workflow checks passed; focused 1,833/1,833 in 6.661 s, fuzzy 28/28 in 22.321 s, one ignored finding; 68 targeted tests passed in 0.054 s |
| 04a1 bounded traversal correction | merged, `0eb1ff0` | all four workflow checks passed; focused 1,833/1,833 in 9.325 s, fuzzy 28/28 in 27.457 s, one ignored finding; traversal uses a configured bounded `for` |
| 04a2 fake forge API additions | merged, `6f8a73e` | all four workflow checks passed on the final reviewed tip; focused 1,843/1,843 in 6.235 s, fuzzy 28/28 in 23.993 s, one ignored finding; 78 targeted tests in 0.073 s |
| 05a fleet turns and graces | merged, `c6513cb` | all four workflow checks passed; focused 1,794/1,794 in 7.478 s, fuzzy 27/27 in 22.515 s, one ignored finding; fleet focused serial total 0.144 s and fuzzy 0.508 s |
| 05b versioned channel and typed payloads | merged, `d3cc9c3` | parent review corrected relay byte bounds and added distinct-value fixtures; all four workflow checks passed; focused 2,091/2,091 in 7.184 s, fuzzy 29/29 in 17.783 s, one ignored finding; channel world serial 0.115 s focused and 0.272 s fuzzy before final scalar fixtures |
| 05c1 fake checkout git foundation | merged, `41eda2b` | all four workflow checks passed; focused 2,123/2,123 in 5.274 s, fuzzy 30/30 in 21.425 s, one ignored finding; physical-git scenarios verify conflict markers, two parents, graph transfer and conditional pushes |
| 05c2 checkout merge state and ownership | merged, `8e94482` | independent review found no blockers; all four workflow checks passed; focused 2,132/2,132 in 5.396 s, fuzzy 31/31 in 21.190 s, one ignored finding; checkout world serial 0.545 s focused and 0.815 s fuzzy |


01a checks pattern inclusion against independently enumerated names and
authority ordering laws. 05a adds bounded turn admission, commitment
acknowledgements, prefix restoration with adoption, fencing and busy
retry; its separate turn world checks reconnects, restart, ownership,
replay, facts and declared stop bounds. Existing fleet scenarios remain.
The legacy root supplies the first version's defaults and exhaustive
ignore arms for additions.

01b keeps exact accounting through overruns and funding transfers, with
independent expense-ledger tests and durable-generation obligations for
the caller. 03a includes atomic owner bootstrap and keyed requests with
commit cuts; retryable root pressure frees its key without saving a final
answer. 04a1 exercises two-parent ancestry and object transfer, unchanged
merge trees, conditional pushes, converging histories at the store limit,
and measured memory. Existing fake-forge callers supply first-version
defaults.

04a2 adds complete bounded file/comparison reads, clean two-parent updates
with CI on the new head, conflict refusal without mutation, adoption
metadata and branch creation through the API. It follows the observed
comparison and job-log fallbacks. Review corrections cover temporary status
arrays, REST/git transport independence, oversized delayed-write inputs,
and exhaustive matching. New reads are measured at their ownership bounds.
04a is complete; the connector client, policy and top still follow.

01c implements bounded policy lifecycle, strictest action checks, current
funding/task-capacity fitting and proposal coverage. Generic connector facts
are pinned to the effect's full resource and state; accepted effects need
another check in the same root commit. 05b preserves explicit v1 callers
while adding typed v2 schemas, negotiation, bounded unknown-kind skipping,
turns/transcripts, independent bytes and heap checks. Runtime translations
and behavior remain later increments.

Step 01 is complete. Landing checks pin CI to the exact head, apply clean
lineage only where configured, count distinct eligible reviewers, and
combine deployment, project and change gates without weakening refusals.
The root remains responsible for authenticating facts and checking them in
the committing decision. Missing landing facts do not hide an invalid role.

02a implements bounded atomic task batches, dependency admission, lifecycle,
closing, cancellation and validated durable restoration. Creation under a
held closing parent refuses without mutation; result credits survive closing
and recovery. Root integration and messages remain later task increments.

05c is complete: the checkout prepares real fake-git merges, forwards owned
conflict sets, retains the second parent through marker refusal, and advances
expected heads only after successful or verified pushes. Saves leave that
condition intact. Cancellation races settle before releasing the workspace;
counted memory includes maximum retired and live conflict payloads. The real
git protocol's enforcement of these typed contracts remains later work.

Steps 02 through 05 are partially implemented. Task messaging (02b), the
forge client (04b) and worker turns (05d) are in isolated worktrees. The
client API foundation is under review; its required world verification must
pass before any client code reaches main. Steps 06 through 08 remain.

After groundwork, the plan's finer dependencies still apply: tasks needs
authority's value and number shapes; the root's walking skeleton needs
01a–c, 02a–b, 03a and 05a. Runtime payload shapes settle in 05b before
dependent extensions. The legacy engine and its worlds stay green and
frozen until the single cutover.

Step 08 includes domain notes and contractions, followed by writing the
separate plans for store, web, credentials, forge protocol and the engine's
iteration/shell. Implementing those lower layers is later work, as
08-after.md, section 3, specifies.
