# Findings — mechanical landing queue and merge-conflict routing

## Final schema and API choices

- Queue automation is optional `automation` metadata on a queue. It names a
  declared workflow role as `actor`, a primary `transition`, and optionally an
  `on_merge_conflict` fallback transition.
- The automation actor is authorization only. It does not subscribe the role to
  the queue, bind external tools, or require a role-decision process.
- Mechanical ticks run bounded reconciliation/apply first, then execute declared
  automated queues through the normal executor path. The reference `landing`
  queue runs `land_pr` as `mechanical` and routes typed merge conflicts through
  `route_merge_conflict`.
- Native review approval remains aggregate/requested-reviewer based rather than
  head-SHA-scoped. Native CI is current-head-scoped when a backend supplies a PR
  head SHA; reference backends without a head SHA use the portable PR/job filter.

## Query-shape guarantees

- This plan preserves the bounded-reconciliation contract from
  `plans/bounded-reconciliation-and-correlation-lookups/`: normal mechanical
  ticks do not call deep audit and do not issue default all-history issue/PR
  list queries.
- Automated-queue discovery reuses the bounded queue-candidate scanner: explicit
  states, non-empty workflow/queue labels, summary list detail, and lazy signal
  reads only after cheap queue matching.
- Wake-driven mechanical ticks use the same normal bounded tick path as polls.
  Hints are optional, lossy latency accelerators; polling remains sufficient for
  convergence.

## Merge-conflict behavior and limitations

- `MergePullRequest` conflicts are re-read before routing. Already-merged PRs
  continue post-merge label projection; missing/closed targets are stale; only
  still-open, unmerged PRs become `ExecutionError::MergeConflict`.
- The reference fallback removes `landing`, adds `merge-conflict`, and comments
  with engineer guidance. Removing `landing` prevents immediate retry loops and
  lets unrelated approved/green PRs continue landing.
- `resolve_merge_conflict` removes `merge-conflict` and re-adds `landing`
  without requesting another review. The new head still needs fresh green CI
  before mechanical landing retries.
- Forgejo 7.0.x exposes coarse merge rejections. Temper conservatively treats an
  open/unmerged PR after a Forgejo merge `Conflict` as engineer-routable, even
  though a future backend may distinguish textual conflicts from branch-policy
  failures.

## Validation run

- `cargo fmt --all`
- `cargo test -p temper-workflow`
- `cargo test -p temper-runner`
- `cargo test -p temper-testing --test multiprocess`
- `cargo test -p temper-testing --test multi_repo_multiprocess`
- `cargo dev-clippy`
- `cargo dev-check`
- `.cache/forgejo/` was populated, so live Forgejo checks were also run:
  - `cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --nocapture`
  - `cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --nocapture`
  - `cargo test -p temper-testing --test forgejo_webhook_wakeup -- --ignored --nocapture`

No local Forgejo checks were skipped.
