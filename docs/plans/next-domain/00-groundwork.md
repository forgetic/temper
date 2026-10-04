# Step 00: groundwork

Provisional, 2026-10-04. What is done before any new domain code: the
facts the forge connector rests on, checked; dead protocol drafts
deleted; the legacy engine moved aside and frozen; the design's documents
moved to their final place; the one convention every new child domain
needs, recorded; the test budgets measured. Overview and conventions:
README.md.

## 1. What it leaves behind

- Forgejo's answers to the questions of `forge.md`, section 20, recorded
  in that section, so step 04 builds on facts.
- No unexported, unchecked forge drafts in the engine's protocol layer.
- The old engine's five crates and four worlds under `legacy` names,
  every test passing as before.
- `docs/design/domain/` holding the next design; today's domain documents
  saying what they now describe.
- The durable-state convention (README.md, 5.2) in `domain/engine.md`,
  section 5.
- Each world's share of the two suites, in README.md, 5.5.

Nothing in this step adds behaviour.

## 2. Increments

### 00a: Forgejo's facts

The change procedure's way to landing assumes Forgejo behaves in ways
nobody has checked (`forge.md`, section 20). Each is a question for the
conformance tool (`tools/forgejo-conformance`, against the verified
Forgejo 15 binary on a loopback listener and scratch data, as
`docs/development/protocol-implementation.md` prepares it):

| Question | Why it matters | If the answer is no |
|---|---|---|
| Does `POST /repos/{o}/{r}/pulls/{n}/update` (style `merge`) refuse a conflicting update, and how: status, body? | updates are effects, and a conflict is a resolution (`forge.md`, section 10) | the change reads mergeability first, and treats an update that fails as uncertain until read |
| Does the update's push start CI, and does it move the pull request's head as a push does? | a clean update's new head is checked again | the change asks CI another way, or pushes the merge itself |
| Can a pull request's files at its head, and a comparison's files and commits, be listed, paged? | overlap of landings with a goal's changes (`forge.md`, 7.3) | overlap is unknown, so every landing wakes |
| Is a failed Actions job's log readable through the API with temper's token? | CI's output in a repair's brief | the brief carries the status's description and link only |
| Can a branch be created at a commit (`POST /repos/{o}/{r}/branches` with `old_ref_name` a commit)? | a goal's own branch; a deleted branch made again on release | the branch is made by a worker's push, which the design forbids, so this is a design change |
| Is branch protection readable with write (not admin) permission? | what temper may do, read at adoption | adoption records it as unknown, and a refused merge narrows the ceiling (`connectors.md`, 11) |
| Do a merge's `head_commit_id` condition and a retargeted base behave as `docs/design/forge.md` assumes? | merges at exactly the head decided | already checked by the paused forge increment; re-run here |

The answers go into `forge.md`, section 20 (each question moved to a
fact, or to a changed design), and the captured exchanges become
fixtures for `temper-forge-forgejo` in step 08. Markdown and fixtures
only.

### 00b: dead drafts and the paused forge increment

- **Delete** `crates/temper-engine-protocol/src/forge_blocks.rs` and
  `forge_cursor.rs` (unexported, unchecked) and
  `tests/engine/protocol/tests/forge_cursor.rs.draft`.
- **In `docs/development/protocol-implementation.md`:** strike the
  paused increment's work the design drops (typed record blocks, item
  comment times and explicit cursors over records, keyed-creation label
  reconciliation beyond display, wiki hints and their coalescing) and say
  why; keep Forgejo's documents and webhooks and the fake forge's
  protocol, and record the decisions the design settles (the README of
  the design, section 5, last bullet but one).
- Nothing exported changes; the gate runs.

### 00c: the legacy engine moved aside

The renames of README.md, 4.1, in one commit:

- move the five crate directories and the four world directories; set
  their package names and descriptions (each description starts
  "Legacy:");
- in the workspace `Cargo.toml`: members, and the `[workspace.dependencies]`
  entries renamed (`temper-legacy-engine-domain = { path = ... }`, ...);
- in their dependents (`temper-engine-protocol`, `tests/engine/protocol`,
  `tests/worker/protocol`, `tests/agent/domain`, `tests/worker/domain`,
  and the legacy crates and worlds themselves): the dependency keys, and
  the paths in code, by a
  word-bounded substitution (`temper_engine_domain\b` to
  `temper_legacy_engine_domain`; `temper_engine_domain_work\b` to
  `temper_legacy_engine_domain_work`; likewise `plan`, `rules`, `forge`,
  and the four world crates' names). `temper_engine_domain_fleet`,
  `_brief`, `_notes`, `_views` and `_accounts` are not legacy and keep
  their names;
- each legacy crate's root doc comment gains one line: frozen, see
  `docs/plans/next-domain/README.md`, 3.4.

The gate must pass with the same tests, the same counts and the same
traces: a rename that changes a trace changed behaviour. The commit
message lists the renames.

### 00d: the documents

- **Move** `docs/design/next/domain/*.md` to `docs/design/domain/`, and
  replace, in its README, "Until it is adopted, the documents in
  `docs/design/` describe what is built" with: the legacy engine is
  described by `docs/design/engine-domain.md` until the cutover
  (`docs/plans/next-domain/07-cutover.md`); new code is written against
  this directory, and cites it as `domain/<file>.md`.
- **Today's documents** gain a first paragraph each:
  `engine-domain.md`, that it describes the legacy engine, frozen, and is
  deleted at the cutover; `worker-domain.md` and `agent-domain.md`, that
  they describe the worker and the agent as built, which the migration
  extends towards `domain/worker.md` and `domain/agent.md`.
- **This plan's citations** of the next design stay bare file names
  (README.md, top).

### 00e: the durable-state convention

Add to `domain/engine.md`, section 5, a subsection "How a child's records
reach a commit", stating README.md, 5.2 and 5.3: the child's `Stored`,
`Key`, `Save`, `Erase`, `Restore`, `Restored` and `Load`; that a child
never waits for a commit nor holds an output back; that the root tags
outward requests with the last commit made and releases them in order
once it is durable. Markdown only.

### 00f: budgets

Run `cargo nextest run --workspace --profile measure -j 1` for both
suites on an idle machine, and record in README.md, 5.5, each world's
total, the two suites' wall times under their profiles, and the
allotments for the new worlds that follow from the room left. Markdown
only.

## 3. Done when

- 00a to 00f merged; the gate green on 00b and 00c with the test counts
  unchanged;
- `rg 'temper_engine_domain(_work|_plan|_rules|_forge)?\b' crates tests`
  finds nothing outside the legacy crates and worlds;
- `forge.md`, section 20, has no question step 04 depends on left
  unanswered.
