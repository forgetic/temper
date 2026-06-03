# Phase 2 prompt — Candidate query planning and closed-history pruning

## Goal

Stop runner scans from using `IssueQuery::default()` and
`PullRequestQuery::default()` for normal role work. Scans should fetch only
candidate artifacts derived from queue interest:

- open artifacts that might match subscribed queues;
- closed/merged artifacts only when they carry labels that make them candidates
  for a queue or recovery path;
- never unlabelled/uninteresting closed history on every tick.

Phase 1 made signal reads lazy; this phase makes artifact listing lazy enough to
scale with repository history.

## Required reading

- Phase 1 implementation and tests
- `crates/temper-runner/src/scan.rs`
- `crates/temper-workflow/src/compile.rs` (`QueueManifest`, artifact kinds)
- `crates/temper-workflow/src/plan/queue.rs`
- `crates/temper-forge/src/forge.rs` (`IssueQuery`, `PullRequestQuery`)
- `docs/reference/forge-interface.md`

## Implementation tasks

1. Introduce a scan/candidate planner in `temper-runner` or `temper-workflow`.
   Given a compiled workflow, optional role, and scan mode, it should produce
   issue and PR list queries.
2. For normal role scans:
   - restrict to queues subscribed by that role;
   - split issue-target and PR-target queues using artifact kind target;
   - build label conjunctions from `queue.labels` plus each `any_of` branch;
   - deduplicate returned artifacts by stable id/number.
3. For open artifacts, query by state `Open`. If a queue has no useful labels,
   open-all for that artifact target is acceptable as a fallback.
4. For closed/merged artifacts, query only with non-empty queue/recovery label
   sets. Do not issue a closed-all query in normal scans.
5. Add an audit scan mode. It may inspect all configured repos and all
   workflow-labelled active/closed-interest artifacts, but it must still avoid
   unlabelled closed history. The audit mode can run less frequently and can be
   wired into production in Phase 4 if that is cleaner.
6. Preserve exact dependency-target reads: dependency gates should fetch target
   artifacts by number/id when needed, not rely on global closed-history scans.
7. If the portable Forge query structs need new fields (for example scan detail
   or include flags), keep defaults backward-compatible and update the Forge
   interface docs in this phase.

## Tests to add or adjust

- A unit test for the candidate planner proving closed queries always carry at
  least one queue/recovery label.
- A regression with many closed unlabelled issues/PRs proving a role scan does
  not classify them.
- A regression proving a closed/merged PR with a queue label such as `landed` is
  still found and processed.
- A regression proving open unlabeled/all-label-fallback queues still work.
- Backend-neutral tests should use counting/fake Forge handles so the test can
  assert query shapes, not just final behavior.

## Validation

Run at least:

```sh
cargo fmt --all
cargo test -p temper-workflow
cargo test -p temper-runner
cargo test -p temper-testing --test multiprocess
cargo dev-check
```

If public Forge query structs changed, also run backend tests for memory,
filesystem, and Forgejo mock contracts.

## Done when

- Normal scans no longer request all closed issues/PRs.
- Closed workflow-active artifacts are still processed via explicit labels.
- The scan planner has tests that document the intended query shape.
- This plan README is updated with Phase 2 status and notable findings.
