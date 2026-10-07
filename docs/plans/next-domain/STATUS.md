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
| Completion 14.3: preserve the notes child | merged | this commit | Restored the notes crate and its world, which step 08, section 1 converts in place later; neither belongs to the protocol or legacy deletion. Gate evidence is in the commit message. |
| Completion 14.2: original test budgets | merged | this commit | With legacy and protocol gone, the focused cap returns to 15 s and the fuzzy cap to 60 s; slow periods return to 3 s and 20 s. Gate and serial world measurements are in this commit message. |
| Completion 14.1: legacy and protocol deletion | merged | this commit | The user’s 2026-10-07 decision brings 07d forward and voids 07a–07c. The last main commit with the deleted code is `655776de7478dea4719e8b3691173c30a6a08861`; step 07’s protocol and legacy instructions no longer apply. Removed their crates and worlds; retained the typed worker runtime and system worlds. Gate: fmt, clippy, 1,092 focused / 5.232 s, 24 fuzzy / 3.409 s; before deletion 2,621 focused / 20.123 s, 45 fuzzy / 57.075 s. |
| Completion 13: root strict subset | merged | this commit | Replaced 23 own-enum or tuple conditional forms with exhaustive matches, including the added proposal and forge routes; root 23→0. Gate: fmt, clippy, 2,621 focused / 13.867 s, 45 fuzzy / 38.339 s. |
| Completion 13: forge let-chain follow-up | merged | this commit | Replaced the overlooked branch-resource `let` chain with an exhaustive match; forge 1→0 remaining forms. Gate: fmt, clippy, 2,621 focused / 12.529 s, 45 fuzzy / 40.396 s. |
| Completion 13: tasks strict subset | merged | this commit | Replaced 18 own-enum conditional forms with exhaustive matches and bounded requester ancestry by the task depth; tasks 18→0 forms and one loop→0. Gate: fmt, clippy, 2,621 focused / 15.631 s, 45 fuzzy / 46.118 s. |
| Completion 13: forge strict subset | merged | this commit | Replaced six own-enum `if let` forms with exhaustive matches; a separate `let` chain remained. Gate: fmt, clippy, 2,621 focused / 13.634 s, 45 fuzzy / 36.775 s. |
| Completion 13: authority strict subset | merged | this commit | Replaced the own-enum `if let` with an exhaustive match; authority 1→0 forms. Gate: fmt, clippy, 2,621 focused / 11.189 s, 45 fuzzy / 30.697 s. |
| Completion 12.2: owners amend the full mutable policy | merged | this commit | Keyed owner edits change project period spend, an existing role's authority/allotment/decisions, or branch landing rules. Authority validates each candidate under deployment rules; the root commits one full mutable policy value and restores it after restart without changing the repository-derived ceiling. Stories cover an owner-added review gate, forbidden spend, restart, and future-only effects. Gate: fmt, clippy, 2,621 focused / 11.525 s, 45 fuzzy / 29.267 s. Root serial: 169 / 1.718 s (prior 166 / 1.756 s); people serial: 14 / 0.062 s. |
| Completion 12.1: adoption as a keyed owner request | merged | this commit | Adoption enters through the people's authenticated request key, reaches the forge connector, commits the collaborator seed and answer together, and replays after a restart without another forge call. The direct adoption event and terminal are gone. Gate: fmt, clippy, 2,618 focused / 12.471 s, 45 fuzzy / 35.767 s. Root serial: 166 / 1.756 s (prior 165 / 1.692 s); people serial: 14 / 0.058 s. |
| Completion 12.0: Smith 06 repin | merged | this commit | Smith `49434506c30758b3d59d729b53f7c718b18b01e4`; Skein stays `4e78d26fc247bf5e7c18300ff8ffc4bd3b06819c` in both repositories. The typed Smith start passes no recovered answered calls until Temper's host retains them; system transcript fixtures use Smith's current turn shape. Gate: fmt, clippy, 2,617 focused / 12.338 s, 45 fuzzy / 30.187 s. |
| Completion 11.3: configured checks without CI | merged | this commit | Adoption explicitly records CI availability and refuses a repository without CI and checks. A change without CI runs its blocking check gates at each head, then the authority merge effect requires their passed verdicts at the exact merged head. Root stories cover landing and repair; authority tests reject stale and failed checks. Gate after rebase: fmt, clippy, 2,617 focused / 15.460 s, 45 fuzzy / 67.877 s (concurrent build). Root serial: 165 / 1.692 s before contention (prior 163 / 1.526 s; original baseline 1.382 s). |
| Completion 11.2: evidence in repair and resolution briefs | merged | this commit | CI briefs carry failed status description, link and bounded job-log tail, marking truncation or an unread log; review remarks, semantic base changes and conflicting files route through bounded connector reads. Root and Smith system stories cover repair and resolution. Gate: fmt, clippy, 2,613 focused / 12.863 s, 45 fuzzy / 28.214 s. Root serial: 163 / 1.526 s (prior 162 / 1.705 s; original baseline 1.382 s). |
| Completion 11.1: gate runs and review repair | merged | this commit | Project landing gates run at the named head; their bounded pull brief reads the forge item, files and comparison, and a failed gate's remarks reach its repair. Root stories cover approval landing and repair followed by a second review. Gate: fmt, clippy, 2,612 focused / 12.797 s, 45 fuzzy / 31.666 s. Root serial: 162 / 1.705 s (baseline 1.382 s). |
| Completion 09.3: typed system world | merged | 05097532 | Seven required stories pass across the real Temper root, Smith fake agent and fake Forgejo; the world also covers the host bridge and a delegated producer. Gate on rebased tip: fmt, clippy, 2,610 focused / 11.625 s, 45 fuzzy / 26.186 s. Nine system tests take 0.130 s serial. |
| Completion 09: corrected Smith host-feedback repin | merged | c9ec5060 | Smith `1697a6d1fe289e5862747a5fdd21f45f2c109794` includes the named `message` bridge test in its agent world. Gate: fmt, clippy, 2,601 focused / 12.222 s, 45 fuzzy / 26.652 s. |
| Completion 09: Smith host-feedback repin | merged | 7e544c09 | Smith `80c9e205058dff56d92cbc0fcc76d2b319ac1ea2` includes the first host-feedback correction. Gate: fmt, clippy, 2,601 focused / 12.397 s, 45 fuzzy / 26.302 s. |
| Completion 09: Smith host-bridge repin | merged | 256ba4ea | Smith `520ccaeaf304ad35421964179823eade4f717308` includes the test-world host bridge. Gate: fmt, clippy, 2,601 focused / 10.137 s, 45 fuzzy / 26.104 s. |
| Completion 09.2: typed Smith translation | merged | 42954d62 | Root assignments carry bounded Smith-neutral run charters; the pure translation crate maps starts, 31 host tools (including one per forge read), typed calls, answers, turns and terminals. Gate: fmt, clippy, 2,601 focused / 12.173 s, 45 fuzzy / 26.552 s. |
| Completion 09.1: Smith domain dependencies | merged | 57981a4c | All four Smith domain packages resolve from Smith main `bf3609f640f05e47e886192b9ff85183a3d12ba1` through the local redirect, with forge URLs retained; both repositories use Skein `4e78d26fc247bf5e7c18300ff8ffc4bd3b06819c`. Gate: fmt, clippy, 2,596 focused / 11.877 s, 45 fuzzy / 27.526 s. |
| Completion 08.4: budgets and legacy map | merged | this commit | `docs/development/legacy-domain-map.md` maps every step 07 section 2 story and rule. The root fake store moves committed writes without full history copies. Gate: fmt, clippy, 2,596 focused / 12.057 s, 45 fuzzy / 27.577 s. Serial shares are below. |
| Completion 08.3: ending sweep and memory | merged | this commit | Root fuzzy sweep covers durable done, failed and cancelled endings plus a held route; the tasks sweep already injects drawn restarts, and root, tasks, people and forge have counted worst-case memory tests. Gate: fmt, clippy, 2,596 focused / 11.730 s, 45 fuzzy / 27.448 s. |
| Completion 08.2: referee test lockfile | merged | this commit | Lock the two new world test dependencies from 08.2. Gate: fmt, clippy, 2,596 focused / 11.913 s, 44 fuzzy / 26.674 s. |
| Completion 08.2: whole referee sensitivity | merged | this commit | Seven negative boundary tests cover authority, once, commit fencing, order, no loss, no overwrite and bounds; the good control runs the real root, while the forge fake observes and refuses a divergent push. Gate: fmt, clippy, 2,596 focused / 12.075 s, 44 fuzzy / 26.795 s. |
| Completion 08.1: every restart cut | merged | this commit | Root world covers lost commit, durable commit before release, an outbox write before its outcome, outcome before commit, existing turn/answer ACK cuts, and push before worker report. Cold restore admits a procedure attempt without requiring an agent run proof; durable CI subscriptions survive restart. Gate: fmt, clippy, 2,589 focused / 11.844 s, 44 fuzzy / 26.276 s. |
| Completion 08.0: Skein repin | merged | this commit | All nine locked Skein packages use main `4e78d26fc247bf5e7c18300ff8ffc4bd3b06819c`, aligned with Smith. Gate: fmt, clippy, 2,586 focused / 12.112 s, 44 fuzzy / 26.832 s. |
| Completion 00: temporary test budgets | merged | this commit | Focused cap 15→30 s and slow period 3→6 s; fuzzy cap 60→120 s and slow period 20→40 s until legacy deletion. Baseline: 2,365 focused / 12.644 s; 42 fuzzy / 30.381 s. Four-check gate passed: fmt, clippy, 2,365 focused / 10.875 s, 42 fuzzy / 32.725 s. |
| Completion 06: forge client | merged | this commit | Rebased parked `04b2` client and world; top-owned entries and client progress, bounded Forgejo v16.0.5 job-log read. Gate passed: fmt, clippy, 2,445 focused / 7.806 s; 43 fuzzy / 31.597 s. |
| Completion 06: forge change | merged | this commit | Pure change procedure and landing queue policy; gate passed: fmt, clippy, 2,456 focused / 11.655 s; 43 fuzzy / 31.051 s. |
| Completion 06: forge issues | merged | this commit | Pure goal issue projection, keyed milestones and interval-limited body writes; gate passed: fmt, clippy, 2,468 focused / 8.518 s; 43 fuzzy / 26.738 s. |
| Completion 07.1: forge top and calm world | merged | this commit | Top owns repository adoption, holds and writer slots, durable outbox release, change and issue rows, and classified topic news. Its fake-forge world covers calm changes, drift, priorities, projections and commit fencing. Gate: fmt, clippy, 2,560 focused / 11.444 s; 43 fuzzy / 26.315 s. |
| Completion 07.2: forge restarts, drift and load | merged | this commit | Restarted keyed issues and merges are found without repeating writes; head races and own pushes have referee coverage. The fake exposes bounded Forgejo v16 job bytes without changing the frozen legacy API, and the connector world adds replay, facts, memory and a seeded sweep. Gate: fmt, clippy, 2,566 focused / 9.901 s; 44 fuzzy / 26.009 s. |
| Completion 07.3: root forge routes | merged | this commit | Adoption seeds collaborator roles; claims resolve saved repositories, holds and writer slots; checked reads and effects, change landing and repair, goal projections, connector news and release cleanup cross the root. Gate: fmt, clippy, 2,582 focused / 7.646 s; 44 fuzzy / 21.822 s. |
| Completion 07.3: connector news coverage | merged | this commit | Root stories receive pull, CI and participation news; the connector reads CI directly for subscribed heads and retains verdicts across restart. Gate: fmt, clippy, 2,586 focused / 12.067 s; 44 fuzzy / 26.171 s. |
| Completion 01: person words and task inbox | merged | a07b2bcc | Authenticated keyed words wake parked chats or relay to a live run; offered unread prefixes are fenced by the committed turn. Gate passed: fmt, clippy, 2,461 focused / 5.682 s; 43 fuzzy / 23.117 s. |
| Completion 01: parked transcript resume | merged | df815a11 | Configured whole-transcript bound spans attempts; a longer conversation starts fresh with a bounded tail in its brief. Gate passed: fmt, clippy, 2,470 focused / 11.952 s; 43 fuzzy / 29.665 s. |
| Completion 01: failures and saved work | merged | d345b6d3 | Classified failures back off then hold, a refused assignment spends no try, and saved-work repository tags persist into the next assignment. Gate passed: fmt, clippy, 2,472 focused / 11.269 s; 43 fuzzy / 30.064 s. |
| Completion 01: lost and frozen workers | merged | fc2e5658 | Root rejects stop bounds reaching engine grace; a lost run resumes from its committed turn only after that grace. Existing worker worlds cover watchdog, cancel and kill. Gate passed: fmt, clippy, 2,474 focused / 12.931 s; 43 fuzzy / 34.895 s. |
| Completion 02: named calls and replay | merged | 8fa1f4ac | Root commits named call answers before fleet delivery, restores records across restart, gives a lost attempt's next run calls answered since its last turn and refuses calls under pressure for retry. Gate on rebased tip: fmt, clippy, 2,477 focused / 11.347 s; 43 fuzzy / 27.312 s. |
| Completion 02: delegate batches and result briefs | merged | d9b07e55 | Root checks authority and historical inputs, makes a whole child batch with the named call answer, routes each ended child's typed result to its requester inbox, and includes live delegates and dependency results in run briefs. Gate on rebased tip: fmt, clippy, 2,485 focused / 11.423 s; 43 fuzzy / 27.932 s. |
| Completion 02: messages and introductions | merged | 1f9e27cb | Root routes named words, questions, answers, and reciprocal introductions through live task references. Question answers use reserved inbox room; introduced sibling dependencies are admitted unless they create a cross-subtree wait cycle. Gate on rebased tip: fmt, clippy, 2,488 focused / 11.611 s; 43 fuzzy / 28.163 s. |
| Completion 02: subscriptions and wake batching | merged | b15d36c4 | Root routes named task and timer interests, task rows retain bounded subscriptions and merge their notices, and wake policies batch a coordinator's inbox across restart. Connector topics join in session 07. Gate on rebased tip: fmt, clippy, 2,491 focused / 11.600 s; 43 fuzzy / 28.542 s. |
| Completion 03: amend, cancel and release | merged | this commit | Root routes named control calls to child; amendments check authority and funding, retain revisions and reach live runs, cancellation closes three levels in depth order, and release resets exhausted tries. Gate: fmt, clippy, 2,495 focused / 11.842 s; 43 fuzzy / 27.966 s. |
| Completion 03: proposals up the tree | merged | dbc3d93c | Durable proposals seek the nearest covering task or person; task and keyed person decisions accept, reject or pass, and stalls pass upward. Gate on rebased tip: fmt, clippy, 2,499 focused / 10.235 s; 43 fuzzy / 27.809 s. |
| Completion 03: escalations up the tree | merged | this commit | Held delegates route to a covering task, pass through two task holders to a person, then release and finish; durable waits stall upward and virtual inbox entries reach live runs. Gate on rebased tip: fmt, clippy, 2,500 focused / 6.118 s; 43 fuzzy / 22.911 s. |
| Completion 03: moves | merged | this commit | A person may adopt a live task they may amend. The old funder keeps spent and returns the remainder, the new pool reserves it in the same decision, the old requester keeps a reference, and pending holders reroute. Gate on rebased tip: fmt, clippy, 2,505 focused / 9.773 s; 43 fuzzy / 27.944 s. |
| Completion 04.1: periods and pools | merged | this commit | Superseded pools and periods retire after their last reservation closes; original-period spend and closed history survive restart. Gate: fmt, clippy, 2,507 focused / 6.469 s; 43 fuzzy / 22.634 s. |
| Completion 04.2: procedure executors | merged | this commit | Fenced procedure steps delegate, wait, hold or finish through the root; routing skips procedure ancestors so a proposal reaches a person. Gate: fmt, clippy, 2,509 focused / 11.668 s; 43 fuzzy / 27.681 s. |
| Completion 04.3: recurring procedure | merged | this commit | Durable recurring templates carve a period allotment and make one batch for the latest due period; skip and wait overlap preserve old settlements. Gate: fmt, clippy, 2,513 focused / 11.774 s; 43 fuzzy / 29.298 s. |
| Completion 04.4: person executors | merged | this commit | The tasks hub keeps person and role addressed tasks, one durable role claim, hand back and contract checked answers; root person requests and inboxes follow in session 05. Gate: fmt, clippy, 2,515 focused / 11.318 s; 43 fuzzy / 27.179 s. |
| Completion 05.1: the whole inbox | merged | this commit | Root pages newest-first task-derived waiting entries and unread results from the store; people caches bounded references, and a completed page chain advances the durable read position. Gate: fmt, clippy, 2,518 focused / 11.021 s; 43 fuzzy / 27.850 s. |
| Completion 05.2: person tasks from the web | merged | this commit | Agent and procedure delegation can create person tasks under role grants; root restores them and routes keyed authenticated take, hand back and answer requests, with durable claims and contract checks in tasks. Gate: fmt, clippy, 2,519 focused / 10.708 s; 43 fuzzy / 28.644 s. |
| Completion 05.3: person requests | merged | this commit | Authenticated goals, proposals, numbered answers, priority changes, amendments, stop, cancel and release route to tasks with durable keyed outcomes. Gate on pre-rebase tip: fmt, clippy, 2,528 focused / 12.069 s; 43 fuzzy / 28.341 s. |
| Completion 05.4: watches through views | merged | this commit | Authenticated run, task-tree and project-goal watches use the views child; committed turns and later task phase or priority changes reach matching watchers after commit, including restored runs. Gate: fmt, clippy, 2,532 focused / 11.332 s; 43 fuzzy / 26.506 s. |
| Completion 05.5: policy, pools and keys | merged | this commit | Owners change role period ceilings for later decisions and administer finite current-period pools; completed keyed answers expire after configured retention, including restored pages. Gate: fmt, clippy, 2,536 focused / 10.998 s; 43 fuzzy / 27.243 s. |
| Skein OAuth consumer handoff (2.5) | merged | this commit | Markdown only; consumer adoption is recorded under 08 below. |
| Alignment 01: grant patterns | merged | fd61587b | Two terminal forms; exact grants cover one name, open grants cover matching descendants. Gate passed: 2,366 focused / 11.760 s; 42 fuzzy / 29.817 s. |
| Alignment 02: lost run | merged | 4e05dff2 | Root records every lost claim as `Failed(Lost)` with one lost try, including no-turn and release-recovery cuts. Gate passed: 2,366 focused / 5.896 s; 42 fuzzy / 27.617 s. |
| Alignment 03: funding state | merged | ab12cd84 | Removed parked-move allotment generations, historical spend and closure rows; ended rows and source postings remain atomic. Gate passed: 2,366 focused / 11.181 s; 42 fuzzy / 28.374 s. |
| Alignment 04: slice records and citations | merged | d4c6e280 | Current slice limits and result-delivery stopgap recorded below; code citations repointed to restored sections or dropped. Gate passed: 2,366 focused / 7.141 s; 42 fuzzy / 27.745 s. |
| Alignment 05: pure grant order | merged | 7fb2f3ea | Removed a terminal-byte clone from the exact grant inclusion query, preserving its no-allocation contract. Gate passed: 2,366 focused / 10.860 s; 42 fuzzy / 28.365 s. |
| Alignment 06: parked move helpers | merged | e2d05d94 | Removed authority's unused move-funding values, transfer function and move-only tests; ordinary carve, charge and settle remain. Gate passed: 2,362 focused / 10.689 s; 42 fuzzy / 28.582 s. |
| Alignment 07: relaxed domain code docs | merged | this commit | Removed field citation stamps and redundant field prose across the new engine and worker domains; module/type citations and real bounds remain. Four-check gate passed: fmt, clippy, 2,362 focused, 42 fuzzy. |
| Alignment 08: release every hold | merged | this commit | Release restores the held phase, resets tries, and rejudges due work; a still-expired deadline creates a fresh hold and escalation. Four-check gate passed: fmt, clippy, focused and fuzzy. |
| Alignment 09: derived result inbox | merged | this commit | Ended tasks carry root-issued commit order; people restores each person's monotonic read position. Authenticated bounded result pages and named reads commit that position with the reply; live notices remain. Four-check gate passed: fmt, clippy, focused and fuzzy. |
| Alignment 10: Skein repin | merged | this commit | All nine locked Skein packages use main `82064688da6ddb2befb777bb310f1b1d27db6956`; protocol routes and worlds account for new native process IO variants. Four-check gate passed: fmt, clippy, focused and fuzzy. |
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
| 05s conformance 8: reserved names | merged locally | smith 180629e | fmt/clippy pass; 539 focused / 8.033 s; 11 fuzzy / 7.642 s. |
| 05s conformance 4: check discovery | merged locally | smith a81a9de | fmt/clippy pass; 541 focused / 6.053 s; 11 fuzzy / 5.602 s. |
| 05s conformance 3: failed directory | merged locally | smith e11f2af | fmt/clippy pass; 543 focused / 3.550 s; 11 fuzzy / 4.630 s. |
| 05s conformance 1: configured endpoints | merged locally | smith 5cf93de | fmt/clippy pass; 544 focused / 3.668 s; 11 fuzzy / 4.434 s. |
| 05s conformance 2: typed host refusal | merged locally | smith 81d1050 | fmt/clippy pass; 544 focused / 3.787 s; 11 fuzzy / 4.605 s. |
| 05s conformance 9: failure kinds | merged locally | smith e85b945 | fmt/clippy pass; 544 focused / 3.510 s; 11 fuzzy / 4.489 s. |
| 05s conformance 6: host retry | merged locally | smith 78cce42 | fmt/clippy pass; 544 focused / 3.797 s; 11 fuzzy / 4.334 s. |
| 05s conformance 7: oversized host answer | merged locally | smith 5ad9630 | fmt/clippy pass; 546 focused / 4.664 s; 11 fuzzy / 5.946 s. |
| 05s conformance 10: session ready list | merged locally | smith 9d4b91a | fmt/clippy pass; 546 focused / 3.300 s; 11 fuzzy / 4.466 s. |
| 05s conformance 5: child workspace tools | merged locally | smith 51fe873 | fmt/clippy pass; 544 focused / 3.368 s; 11 fuzzy / 4.408 s. |
| 05s8 local host 2.1: crate and charter | merged locally | smith 3efed66 | fmt/clippy pass; 547 focused / 3.376 s; 11 fuzzy / 4.335 s. |
| 05s8 local host 2.2: in-process agent | merged locally | smith 1f06f6d | fmt/clippy pass; 548 focused / 3.254 s; 11 fuzzy / 4.013 s. |
| 05s8 local host 2.3: transcripts and resume | merged locally | smith 361dd90 | fmt/clippy pass; 553 focused / 3.797 s; 11 fuzzy / 5.818 s. |
| 05s8 local host 2.4: credentials, cancel and pressure | merged locally | smith 1276f7b | fmt/clippy pass; 557 focused / 3.526 s; 11 fuzzy / 4.548 s; local serial 10 / 0.044 s. |
| 05s8 local host 2.5: referee, replay and randomness | merged locally | smith 7d98378 | fmt/clippy pass; 568 focused / 7.830 s; 12 fuzzy / 9.597 s; local serial 21 / 0.087 s focused and 1 / 0.033 s fuzzy. |
| 05s8 local host 3.1: workspace in place | merged locally | smith 01e870e | fmt/clippy pass; 569 focused / 7.397 s; 12 fuzzy / 7.272 s; local serial 22 / 0.179 s. |
| 05s8 local host 3.2: delivery in place | merged locally | smith 1cf1d29 | fmt/clippy pass; 583 focused / 12.521 s; 12 fuzzy / 10.007 s; local serial 34 / 0.288 s. |
| 05s8 local host 3.3: configured push | merged locally | smith 844d4c3 | fmt/clippy pass; 585 focused / 3.738 s; 12 fuzzy / 4.550 s; local serial 36 / 0.166 s. |
| 05s8 local host 05: delivery intent and restart reconciliation | merged locally | smith 5164339 | fmt/clippy pass; 591 focused / 3.366 s; 12 fuzzy / 4.301 s; local serial 41 / 0.185 s focused and 1 / 0.035 s fuzzy. |
| 05s8 local host 05: waking notice read fence | merged locally | smith c3ff7a6 | fmt/clippy pass; 592 focused / 3.630 s; 12 fuzzy / 4.408 s. |
| 05s8 shared Skein OAuth revision | merged locally | smith bf3609f | All 13 Skein packages pinned to 4e78d26; fmt/clippy pass; 592 focused / 3.871 s; 12 fuzzy / 4.457 s. |
| 05s8 agent-world parent host-call bridge | merged locally | smith 520ccae | Caller-supplied complete Start and pending host calls receive caller replies while default fixtures remain scripted; fmt/clippy pass; 593 focused / 3.593 s; 12 fuzzy / 4.490 s. |
| 05s8 agent-world host feedback for caller names | merged locally | smith 80c9e20 | The wire world observes any decoded host call, including `message`; fmt/clippy pass; 594 focused / 3.624 s; 12 fuzzy / 4.506 s. |
| 05s8 typed agent-world host feedback for caller names | merged locally | smith 1697a6d | The typed bridge observes the caller-declared `message` host call and its continuation feedback; fmt/clippy pass; 595 focused / 3.770 s; 12 fuzzy / 4.493 s. |
| 05s8 local host 6.1: named answers on agent start | merged locally | smith 5685caf | fmt/clippy pass; 596 focused / 6.661 s; 12 fuzzy / 6.954 s. |
| 05s8 local host 6.1b: waking prompt answers | merged locally | smith e685b15 | Settled turns retained; each absent host answer is rendered in the waking prompt. fmt/clippy pass; 597 focused / 11.833 s; 12 fuzzy / 5.491 s. |
| 05s8 local host 6.2: durable answers by call name | merged locally | smith f9be9a8 | Multiple intents and answers survive restart until a following turn is saved; the run renders every outcome and keeps the full person line. fmt/clippy pass; 599 focused / 5.414 s; 12 fuzzy / 6.042 s; local serial 45 / 0.211 s. |
| 05s8 local host 6.3: configured push recovery | merged locally | smith c5b1c11 | Saved targets drive recovery after commit and on either side of the push terminal; remote stale and failure outcomes are covered. fmt/clippy pass; 605 focused / 4.100 s; 12 fuzzy / 5.156 s; local serial 51 / 0.246 s. |
| 05s8 local host 6.4: moved local head is stale | merged locally | smith b1557b8 | A run retains each repository's start head and its own later commits; an outside move stops delivery before any commit. fmt/clippy pass; 607 focused / 5.136 s; 12 fuzzy / 5.769 s; local serial 53 / 0.261 s. |
| 05s8 local host 6.5: bounded request routing | merged locally | smith 4943450 | Local child requests drain with a traversal bounded by `max_out`. fmt/clippy pass; 607 focused / 6.394 s; 12 fuzzy / 7.445 s. |

## Completion 08.4 measured suite shares

On an idle machine, `cargo nextest run --workspace --profile measure -j 1`
selected the focused suite, and `--ignore-default-filter -E 'binary(/^fuzzy_/)'`
selected the fuzzy suite. These are serial test durations; the gate's
parallel wall times were 12.057 s focused and 27.577 s fuzzy, under the
30 s and 120 s suite caps in the completion plan's README, section 5.5.

| World or share | Focused tests / serial seconds | Fuzzy tests / serial seconds |
|---|---:|---:|
| New root | 160 / 1.382 | 7 / 3.150 |
| New tasks | 55 / 0.332 | 1 / 1.866 |
| New people | 14 / 0.068 | 1 / 0.085 |
| New forge top | 28 / 0.149 | 1 / 0.022 |
| Legacy crates and worlds | 729 / 10.979 | 13 / 17.342 |
| Other crates and worlds | 1,610 / 16.210 | 22 / 32.701 |
| Whole suite | 2,596 / 29.192 wall | 45 / 55.167 wall |

The new world's allotment is 1 s focused and 8 s fuzzy each while legacy
runs. Root's focused serial share is 0.382 s above that allotment; the other
new worlds fit, and both enforced suite caps pass. The root fake store's
write transfer reduced its share from 1.926 s before this increment. This
remaining root share is a test-cost gap for later consolidation or faster
simulation; no legacy fuzzy seed trim is needed for the suite caps.

## Alignment slice limits

- The root rejects restored non-person requesters, non-Report contracts, and delegates until it has routes for them.
- Role administration reroutes held chats only.
- Escalation selects the role named by project policy (domain/tasks.md, section 8).
- Person-requested results are derived from committed ended tasks in result order. People caches bounded unread references and persists each person's read position; older entries are paged. Questions, proposals, person tasks and chat replies remain for 06d.

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
- Skein's OAuth client and fake issuer are complete; consumer adoption remains in credential work 08.

**06 — root**

- Continue 06b–f: run, tool, people and forge routes; broader restarts and worst-case worlds.
- Cover the remaining run, tool and connector restart cuts in 06b–f.

**07 — cutover**

- Verify the new domains together through typed system worlds.
- Keep protocol-dependent service cutover deferred; preserve frozen legacy until its later single cutover.

**08 — after**

- At credential work 08, move `temper-oauth` to `skein-oauth`. Temper's web uses it to sign people in; the future Smith binary uses it for local-host sign-in.
- Implement notes and contractions after cutover.
- Write separate store, web, credentials, forge protocol and iteration/shell plans.
- Implementing those lower layers remains outside this migration.

## Resume point

- Smith's domain alignment plan has merged its domain work and shared Skein repin locally through `b607cd8`. The audit against smith's `docs/design/domain/` found two remaining departures: same-step continuation after immediate tool answers, and conditional check discovery for writable directories. Incomplete features may remain. Smith protocol changes are outside this goal.
- Existing IO/TLS/channel checkpoints are merged; protocol adoption and codec implementation remain deferred.
- Kernel gates pass after restart; browser repair remains deferred in its separate profile.
- Keep parked 02e/04b2/05f branches, exact-tip gates and Forgejo v16.0.5 API job-log assumption.

## smith protocol layer

| Increment | State | Commit | Evidence |
|---|---|---|---|
| 00.1 activation and conversation turn numbers | merged locally | smith b08b00e | fmt/clippy pass; 608 focused / 8.768 s; 12 fuzzy / 9.624 s. |
| 00.2 acknowledgement window | merged locally | smith 49f6609 | fmt/clippy pass; 611 focused / 6.679 s; 12 fuzzy / 5.857 s. |
| 00.3 checks-ended notice | merged locally | smith 87ff00c | fmt/clippy pass; 612 focused / 6.341 s; 12 fuzzy / 6.111 s. |
| 00.4 message label and text | merged locally | smith 704f2f6 | fmt/clippy pass; 613 focused / 5.254 s; 12 fuzzy / 6.032 s. |
| 00.5 unsupported charter version | merged locally | smith 46e7eac | fmt/clippy pass; 615 focused / 8.997 s; 12 fuzzy / 6.021 s. |
| 01.0 skein codec repin | merged locally | smith 3c6c1fc | fmt/clippy pass; 615 focused / 7.568 s; 12 fuzzy / 6.565 s. |

## jig extraction

| Increment | State | Commit | Evidence |
|---|---|---|---|
| jig 00.1 boundary check | merged and pushed | fcad6a05 | Checks future jig manifests and source vocabulary, with breach fixtures. Gate: fmt/clippy; 1,138 focused / 2.343 s; 25 fuzzy / 2.750 s. |
| jig 00.2 world helpers | already satisfied | — | `temper-world` reexports the generic schedule, stage, ledger, trace, referee and heap machinery from `skein-world`; jig's worlds can use it directly. |
| jig 00.3 accounts | merged and pushed | c4999d26, 6dca842e, b2a4da03 | Carved the accounts child and its world, with history; the world uses `skein-world` for heap measurement. Gate: fmt/clippy; 1,138 focused / 3.170 s; 25 fuzzy / 4.001 s. |
