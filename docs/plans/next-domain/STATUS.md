# Migration status

Resumed at the user’s request on 2026-10-05 with the joint temper/smith
goal. Local branches only; nothing has been pushed. Counts below are the full
workspace gate for merged rows; temporary rows distinguish component diagnostics
from full consumer checks. Temper's fuzzy runs retain one ignored finding.
Detailed implementation and review evidence belongs in commit messages.
The focused fixture gate skips its two explicit regeneration tests.

The goal covers Temper and standalone Smith domain logic, including their
typed domain integration, as [README.md](README.md) states. The user clarified
on 2026-10-06 that domain implementation requires no protocol changes;
protocol drafts and lower-layer integration are parked for later work.

| Increment | State | Commit | Evidence |
|---|---|---|---|
| Alignment 01: grant patterns | merged | fd61587b | Two terminal forms; exact grants cover one name, open grants cover matching descendants. Gate passed: 2,366 focused / 11.760 s; 42 fuzzy / 29.817 s. |
| Alignment 02: lost run | merged | 4e05dff2 | Root records every lost claim as `Failed(Lost)` with one lost try, including no-turn and release-recovery cuts. Gate passed: 2,366 focused / 5.896 s; 42 fuzzy / 27.617 s. |
| Alignment 03: funding state | merged | ab12cd84 | Removed parked-move allotment generations, historical spend and closure rows; ended rows and source postings remain atomic. Gate passed: 2,366 focused / 11.181 s; 42 fuzzy / 28.374 s. |
| Alignment 04: slice records and citations | merged | d4c6e280 | Current slice limits and result-delivery stopgap recorded below; code citations repointed to restored sections or dropped. Gate passed: 2,366 focused / 7.141 s; 42 fuzzy / 27.745 s. |
| Alignment 05: pure grant order | merged | 7fb2f3ea | Removed a terminal-byte clone from the exact grant inclusion query, preserving its no-allocation contract. Gate passed: 2,366 focused / 10.860 s; 42 fuzzy / 28.365 s. |
| Alignment 06: parked move helpers | merged | e2d05d94 | Removed authority's unused move-funding values, transfer function and move-only tests; ordinary carve, charge and settle remain. Gate passed: 2,362 focused / 10.689 s; 42 fuzzy / 28.582 s. |
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
| 05s2a native tool identities | merged locally; Smith adapter open | skein 2c725c2 | Gate passed; 1,052 focused / 2.785 s; 64 fuzzy / 16.281 s; exact continuation and counted maximum IDs. |
| 05s2a identity revision (Smith) | merged locally; adapter open | smith 03d1b40 | Gate passed; 459 focused / 1.739 s; 10 fuzzy / 2.962 s; all nine canonical Skein packages use 2c725c2. |
| 05s2a identity revision (Temper) | merged | 9e86d6d4 | Gate passed; 2,292 focused / 10.339 s; 40 fuzzy / 26.092 s; all nine canonical Skein packages use 2c725c2. |
| 02d1 actual escalation routes | merged; broader escalation open | 754ca84 | Gate passed; 2,277 focused / 5.850 s; 39 fuzzy / 24.314 s; root serial 68 / 0.393 s, 5 / 1.293 s. |
| 03c role administration and live rerouting | merged; narrow dependency | 6dfa4d2 | Gate passed; 2,292 focused / 10.993 s; 40 fuzzy / 26.798 s; root serial 78 / 0.490 s, 6 / 1.435 s. |

The 2026-10-06 restart restored writable repositories and kernel access.
The checkpoints below are merged into the original Smith and Skein mains;
all saved drafts also have local `checkpoint/*` branches in the original
repositories. Smith's final source gate passed 589 focused / 5.126 s and
11 fuzzy / 5.575 s; Skein's combined gate passed 1,206 focused / 4.982 s
and 71 fuzzy / 29.694 s. Formatting, all-target Clippy and isolated TLS
compilation passed. Real browser tests remain in their separate profile,
with repair deferred. Exact logs, reviews and hashes remain in
`target/next-domain-handoff/evidence/`.

Protocol checkpoint merging preserves previously authored work; domain
implementation remains the goal. Transcript manifests without Rust targets
remain on `checkpoint/migration/transcript-codec`, outside main.

| Increment | State | Commit | Evidence |
|---|---|---|---|
| 05s2a bounded raw argument history | merged | skein a164071 | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy pass. |
| 05s2a synchronized shared iteration clock | merged | skein 39775d2 | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy pass. |
| 05s2a preserved shared fake mechanics | merged | skein ee224ef | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy pass. |
| 05s2a prepared raw Client adoption | merged | skein 843edad | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy pass. |
| 05s2a bounded native peer ownership | merged | skein 43bba37 | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy pass. |
| 05s2a shared adapter and V2 messages | merged | smith 9b7990a | fmt/clippy pass; 489 focused / 1.895 s; 10 fuzzy / 4.362 s. |
| 05s2a Smith public boundary documentation | merged | smith edc583e | fmt/clippy pass; 489 focused / 1.937 s; 10 fuzzy / 4.294 s. |
| 05s2a actual Client through root routes | merged | smith 240838d | fmt/clippy pass; 490 focused / 1.983 s; 10 fuzzy / 4.310 s. |
| 05s2a adapter owner and inventory controls | merged | smith 9b8630e | fmt/clippy pass; 496 focused / 1.968 s; 10 fuzzy / 4.219 s. |
| 05s4 actual submitted delivery and host ACKs | merged | smith 9d2ec84 | fmt/clippy pass; 498 focused / 1.930 s; 10 fuzzy / 4.321 s. |
| 05s4 attained root and caller-copy ownership | merged | smith 8c51dff | fmt/clippy pass; 499 focused / 2.047 s; 10 fuzzy / 4.305 s. |
| 05s4 observed bounded message sweep | merged | smith a54edcb | fmt/clippy pass; 499 focused / 2.141 s; 11 fuzzy / 4.478 s. |
| 05s4 native continuation and host origin | merged | smith 8010bf4 | fmt/clippy pass; 504 focused / 2.103 s; 11 fuzzy / 4.403 s. |
| 05s4 combined native root/caller memory | merged | smith 1a0bdfe | fmt/clippy pass; 504 focused / 2.097 s; 11 fuzzy / 4.445 s. |
| 05s2a exclusive actual provider backend | merged | smith c645669 | fmt/clippy pass; 504 focused / 2.094 s; 11 fuzzy / 4.410 s. |
| 05s4 genuine native root restore | merged | smith 5b4bc94 | fmt/clippy pass; 505 focused / 2.285 s; 11 fuzzy / 4.958 s. |
| 05s4 overlapping physical Client ownership | merged | smith f9c9981 | fmt/clippy pass; 505 focused / 2.170 s; 11 fuzzy / 4.462 s. |
| 05s4 caller conventions | merged | smith 48dba1d | fmt/clippy pass; 512 focused / 2.233 s; 11 fuzzy / 3.993 s. |
| 05s4 optional workspace and merge conflicts | merged | smith 37599ef | fmt/clippy pass; 522 focused / 2.114 s; 11 fuzzy / 4.021 s. |
| 05s4 instructions and titled brief | merged | smith 440e87d | fmt/clippy pass; 529 focused / 2.145 s; 11 fuzzy / 3.910 s. |
| 05s4 host-unit budget and completion gate | merged | smith 9930191 | fmt/clippy pass; 560 focused / 2.255 s; 11 fuzzy / 3.928 s. |
| 05s4 session first-version contraction | merged | smith 2f324b0 | fmt/clippy pass; 567 focused / 3.138 s; 11 fuzzy / 3.917 s. |
| 05s6 final global host accounting | merged | smith 1b62e06 | fmt/clippy pass; 572 focused / 3.866 s; 11 fuzzy / 4.064 s. |
| 05s6 issued-message refusals | merged | smith d9a7040 | Final checkpoint gate: fmt/clippy pass; 589 focused / 5.126 s; 11 fuzzy / 5.575 s. |
| 05s5 durable recovery design | reverted | smith 39f51f3, 5aa22cb | Outside the 05s plan; no implementation scheduled. |
| 05s5 native IO output | checkpoint merged; adoption deferred | skein 2888b0d | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy and isolated TLS pass. |
| 05s5 native TLS output | checkpoint merged; adoption deferred | skein 8c5455f | Combined checkpoint gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy and isolated TLS pass. |
| 05s5 shared framing | checkpoint merged; adoption deferred | skein a423734, 8206468 | Combined gate: 1,206 focused / 4.982 s; 71 fuzzy / 29.694 s; fmt/clippy pass. |
| 05s5 full transcript codec | draft moved to scratch; source absent | smith bbe922f | No codec implementation or tests; incomplete manifests remain on checkpoint branch. |
| 05s4 activation-qualified host calls | merged locally | smith 5aa22cb, 774f618 | Final-tip fmt/clippy pass; 591 focused / 3.954 s; 11 fuzzy / 4.432 s; serial 591 / 9.488 s, 11 / 9.102 s. |

## Alignment slice limits

- The root rejects restored non-person requesters, non-Report contracts, and delegates until it has routes for them.
- Release reopens only holds caused by exhausted retries; other holds answer `NeedsAmend`.
- Role administration reroutes held chats only.
- Escalation selects the role named by project policy (domain/tasks.md, section 8).
- A live notice followed by `ReadResult` of a named ended task is the current result-delivery stopgap. Step 06d must replace it with one committed message to the requester (domain/tasks.md, section 5.6).

## What remains open

**00 — groundwork**

- Target Forgejo v16.0.5; read job logs through its API.
- Preserve v15 observations; capture v16 log exchanges in protocol work.

**01 — authority: complete**

**02 — tasks**

- Finish retired-source and recurring funding depth in 02e.
- Restore inbox, amendment and move surface only with actual root routes; review the parked move-funding policy before 02c resumes.
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

- Audit remaining Run, Session, Tools and Host contracts; keep Smith first and verify every domain increment.
- Keep codecs, channel/protocol, IO/TLS and binaries parked; preserve SDK cleanup gates and frozen legacy rules.

**06 — root**

- Continue 06b–f: run, tool, people and forge routes; broader restarts and worst-case worlds.
- Cover the remaining run, tool and connector restart cuts in 06b–f.

**07 — cutover**

- Verify the new domains together through typed system worlds.
- Keep protocol-dependent service cutover deferred; preserve frozen legacy until its later single cutover.

**08 — after**

- Implement notes and contractions after cutover.
- Write separate store, web, credentials, forge protocol and iteration/shell plans.
- Implementing those lower layers remains outside this migration.

## Resume point

- Smith first: resume its domain alignment plan; protocol changes are outside the goal.
- Existing IO/TLS/channel checkpoints are merged; protocol adoption and codec implementation remain deferred.
- Kernel gates pass after restart; browser repair remains deferred in its separate profile.
- Keep parked 02e/04b2/05f branches, exact-tip gates and Forgejo v16.0.5 API job-log assumption.
