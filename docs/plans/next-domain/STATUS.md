# Migration status

Resumed at the user’s request on 2026-10-05 with the joint temper/smith
goal. Local branches only; nothing has been pushed. Counts below are the full
workspace gate at the listed source tip; temper's fuzzy runs retain one ignored finding.
Detailed implementation and review evidence belongs in commit messages.
The focused fixture gate skips its two explicit regeneration tests.

The revised goal covers both temper's domain migration and the standalone
smith domain in `~/src/rust/smith/`; completion requires both designs in
place and their integration, as [README.md](README.md) states.

| Increment | State | Commit | Evidence |
|---|---|---|---|
| 00a Forgejo facts | merged | fa97784 | Gate passed; 1,772 focused / 7.440 s; 26 fuzzy / 22.148 s. |
| 00b dead drafts | merged | 9acb981 | Gate passed; 1,772 focused and 26 fuzzy; baseline counts unchanged. |
| 00c legacy rename | merged | d8385dc | Gate passed; 1,772 focused / 7.495 s; 26 fuzzy / 22.159 s. |
| 00d design destination | merged | be9164e | Markdown only; design now under `docs/design/domain/`. |
| 00e durable-state convention | merged | 15d5708 | Markdown only; `domain/engine.md`, 5.6. |
| 00f budgets | merged | 8f11611 | Markdown only; measured baseline and initial allotments in README.md, 5.5. |
| 01a authority values and order | merged | 2d38f2b | Gate passed; 1,778 focused / 7.918 s; 26 fuzzy / 22.367 s. |
| 01b funding arithmetic | merged | c80d438 | Gate passed; 1,829 focused / 5.256 s; 28 fuzzy / 22.473 s. |
| 01c authority policies and checks | merged | e21dafb | Gate passed; 1,850 focused / 4.888 s; 28 fuzzy / 18.143 s. |
| 01d landing requirements | merged | e9aea6d | Gate passed; 2,118 focused / 4.506 s; 30 fuzzy / 16.345 s. |
| 02a tasks batches and lifecycle | merged | 376cbd4 | Gate passed; 2,115 focused / 5.532 s; 30 fuzzy / 20.070 s. |
| 02b task inboxes, references and wakes | merged; dormant surface parked | 1cfd373 | Gate passed; 2,145 focused / 5.042 s; 31 fuzzy / 18.227 s. |
| 02c task amendments and moves | merged; dormant surface parked | ec38fcc | Gate passed; 2,207 focused / 5.086 s; 34 fuzzy / 18.911 s. |
| 02e finite funding seam | seam merged; depth parked | c987a083, 061030e | Gate passed; 2,237 focused / 5.649 s; 36 fuzzy / 20.113 s; tasks serial 65 / 0.356 s, 2 / 3.111 s. |
| 03a people sign-in and chat requests | merged | 959795f | Gate passed; 1,822 focused / 7.251 s; 28 fuzzy / 22.695 s. |
| 04a1 fake forge git foundations | merged | 0d09efc, 0eb1ff0 | Gate passed; 1,833 focused / 9.325 s; 28 fuzzy / 27.457 s. |
| 04a2 fake forge API additions | merged | 6f8a73e | Gate passed; 1,843 focused / 6.235 s; 28 fuzzy / 23.993 s. |
| 05a fleet turns and graces | merged | c6513cb | Gate passed; 1,794 focused / 7.478 s; 27 fuzzy / 22.515 s. |
| 05b versioned channel and typed payloads | merged | d3cc9c3 | Gate passed; 2,091 focused / 7.184 s; 29 fuzzy / 17.783 s. |
| 05c1 fake checkout git foundation | merged | 41eda2b | Gate passed; 2,123 focused / 5.274 s; 30 fuzzy / 21.425 s. |
| 05c2 checkout merge state and ownership | merged | 8e94482 | Gate passed; 2,132 focused / 5.396 s; 31 fuzzy / 21.190 s. |
| 05d worker turns and transcript lifecycle | merged | e2a6a71 | Gate passed; 2,172 focused / 6.761 s; 32 fuzzy / 25.290 s. |
| 05e session transcripts and pricing | merged | 8b91ed3 | Gate passed; 2,187 focused / 5.116 s; 33 fuzzy / 17.917 s. |
| 06a walking skeleton | story merged | 429647c | Gate passed; 2,270 focused / 10.521 s; 38 fuzzy / 29.586 s; root serial 40 / 0.168 s, 3 / 0.740 s. |
| 05s0 smith's design | merged; smith's first commit | 8b32f55; smith 874c4d7 | Markdown only: smith's `docs/design/domain/`; temper's `agent.md` rewritten; this plan revised. |
| 06a tasks boundary audit | merged | c6d07ef | Gate passed; 2,249 focused / 5.861 s; 37 fuzzy / 17.878 s; root serial 46 / 0.190 s, tasks 37 / 0.167 s. |
| Boundary style | merged | 5ca23b7 | Gate passed; 2,249 focused / 8.630 s; 37 fuzzy / 27.153 s. |
| Field-doc formatting companion | merged with backfill | a718c70 | Final backfill gate; 2,249 focused / 11.273 s; 37 fuzzy / 29.489 s. |
| Four-crate documentation backfill | merged | 6f0698a | Gate passed; 2,249 focused / 11.273 s; 37 fuzzy / 29.489 s; 33 files comment-only. |
| Channel golden regeneration | merged | e0aa8f3 | Gate passed; 2,251 focused / 11.393 s; 37 fuzzy / 29.164 s; 218 binaries regenerate unchanged. |
| Shared domain-world kit (skein) | merged locally | skein 3cbd792 | Gate passed; 983 focused / 4.210 s; 62 fuzzy / 23.355 s. |
| Shared domain-world kit (temper) | merged | c51bb5c | Gate passed; 2,251 focused / 10.826 s; 37 fuzzy / 28.978 s. |
| 05s1 smith workspace | merged locally | smith e60ab48 | Gate passed; 2 focused / 0.004 s; 1 fuzzy / 0.006 s. |
| 06a Report terminal commit restart | merged | 2e83cc2 | Gate passed; 2,256 focused / 11.832 s; 38 fuzzy / 29.735 s; root serial 51 / 0.287 s, 4 / 0.927 s. |
| Shared fake checkout (skein) | merged locally | skein 5e52dd9 | Gate passed; 997 focused / 3.824 s; 63 fuzzy / 24.571 s. |
| Shared fake checkout (temper) | merged | 71bdab9 | Gate passed; 2,256 focused / 10.520 s; 38 fuzzy / 29.344 s. |
| 05s2 smith agent copy | merged locally | smith 8a959a5 | Gate passed; 351 focused / 1.435 s; 8 fuzzy / 2.674 s; serial 351 / 4.446 s, 8 / 6.278 s. |
| 05s3 temper legacy agent rename | merged | 669bb52 | Gate passed; 2,256 focused / 10.283 s; 38 fuzzy / 28.851 s; behavior unchanged. |
| 05s4 smith generic results | merged locally | smith 14cd733 | Gate passed; 368 focused / 1.509 s; 8 fuzzy / 2.820 s; serial 368 / 4.359 s, 8 / 6.150 s. |
| 05s4 smith generic delivery | merged locally | smith 2a621a5 | Gate passed; 394 focused / 1.613 s; 9 fuzzy / 2.530 s; serial 394 / 4.429 s, 9 / 6.229 s. |
| 05s4 smith declared host tools | merged locally | smith ab15cae | Gate passed; 411 focused / 1.786 s; 9 fuzzy / 3.111 s; serial 411 / 4.764 s, 9 / 15.212 s. |
| 05s6 smith V2 host supervision | merged locally | smith eb46ecc | Gate passed; 459 focused / 1.606 s; 10 fuzzy / 2.593 s; serial 459 / 5.001 s, 10 / 6.569 s. |
| 05s2a provider-neutral design | merged locally | smith 51fdd24 | Markdown only; actual Skein Client and shared peer ownership; Smith application policy and caller credentials. |
| 05s2a shared LLM core | merged locally; Smith adapter open | skein b2eeae9 | Gate passed; 1,045 focused / 2.777 s; 64 fuzzy / 27.913 s; 28 capture resources preserved. |
| 05s2a prepared Client world | merged locally; Smith adapter open | skein 8b83175 | Gate passed; 1,046 focused / 2.878 s; 64 fuzzy / 17.244 s; caller's one actual Client drives the shared peer. |
| 05s2a shared revision (Smith) | merged locally; adapter open | smith 38c0590 | Gate passed; 459 focused / 3.060 s; 10 fuzzy / 2.973 s; all nine canonical Skein packages use 8b83175. |
| 05s2a shared revision (Temper) | merged | e94e0508 | Gate passed; 2,292 focused / 6.043 s; 40 fuzzy / 24.343 s; all nine canonical Skein packages use 8b83175. |
| 05s2a replay receiving admission | merged locally; Smith adapter open | skein 4bcfe62 | Gate passed; 1,048 focused / 2.757 s; 64 fuzzy / 16.270 s; generated replay obeys the full raw cap. |
| 02d1 actual escalation routes | merged; broader escalation open | 754ca84 | Gate passed; 2,277 focused / 5.850 s; 39 fuzzy / 24.314 s; root serial 68 / 0.393 s, 5 / 1.293 s. |
| 03c role administration and live rerouting | merged; narrow dependency | 6dfa4d2 | Gate passed; 2,292 focused / 10.993 s; 40 fuzzy / 26.798 s; root serial 78 / 0.490 s, 6 / 1.435 s. |

## What remains open

**00 — groundwork**

- Target Forgejo v16.0.5; read job logs through its API.
- Preserve v15 observations; capture v16 log exchanges in protocol work.

**01 — authority: complete**

**02 — tasks**

- Finish retired-source and recurring funding depth in 02e.
- Restore inbox, amendment and move surface only with actual root routes.
- Complete broader 02d task-tree escalation; live membership rerouting now has an actual root route.

**03 — people**

- Complete the remaining 03c policy, pool and adoption routes after people pages and inboxes.
- Resume pages, derived inboxes and the remaining policy/funding work in plan order.

**04 — forge connector**

- Keep 04b–f parked; preserve the in-progress client and review repairs.
- Use bounded Forgejo v16.0.5 API job logs for failed-CI repair briefs.
- Resume client, connector policy and top in plan order after 06a's audit.

**05 — runtime**

- 05f–g replaced by 05s (smith); keep their branch (`next-domain/05f`) to port into 05s4 and 05s5.
- Carry remaining v2 watchdog, cancellation and kill coverage into runtime verification.

**05s — smith**

- Provision smith's forge remotes; preserve the frozen legacy agent until cutover.
- Adopt the merged Skein LLM client and remove Smith's copied provider/OAuth crates; complete messages/wait/parking/resume and remaining run increments (05s2a, 05s4).
- Build channel/protocol, Temper's runtime half and local-host worlds (05s5, 05s7–8); shared kit belongs in Skein.

**06 — root**

- Continue 06b–f: run, tool, people and forge routes; broader restarts and worst-case worlds.
- Cover the remaining run, tool and connector restart cuts in 06b–f.

**07 — cutover**

- Move engine protocol and system worlds after the new root's stories pass.
- Preserve the frozen legacy behavior until the single cutover; then delete it.

**08 — after**

- Implement notes and contractions after cutover.
- Write separate store, web, credentials, forge protocol and iteration/shell plans.
- Implementing those lower layers remains outside this migration.

## Resume point

- Active: Smith's real `skein-llm` adapter and messages/wait/parking/resume. The shared Client/peer/replay changes, generic host tools and V2 host supervision are merged; consumer adoption and copied-crate removal remain open. Smith has priority.
- Parked drafts: `next-domain/02e-depth-draft` (`a60bf1c`), `next-domain/04b2` (`0eff9dc`), `next-domain/05f` (`c33941c`). These have not passed the merge gate.
- Shared skein revision is local and cached; fresh-machine fetches need its authorized publication. Forgejo v16.0.5 job-log client repairs remain open.
