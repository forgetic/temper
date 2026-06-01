# Multi-repo workers — implementation plan

> **Scheduling note:** do not execute this plan until
> `plans/hint-driven-wakeups/` is complete. That plan is actively changing the
> trigger/wake path, and this one must build on the final hint contract rather
> than race it.

Today each `harness-worker` process is bound to one `--repo owner/name`. The
workflow and Forge interfaces are already repository-scoped, so this plan turns
that deployment choice into a fixed worker pool that can process any configured
repository independently.

Hand the prompt files to the agent loop **one phase at a time, in order**. Each
phase should land green and update this README's status.

## Goal

An operator can file a workflow issue in any configured Forgejo repository and a
single fixed set of role/mechanical workers processes it. There is no per-project
worker pool. Repositories are independent: no cross-repo dependencies, routing,
fairness guarantees beyond bounded scanning, or shared workflow state are needed
for this plan.

## Non-goals

- Cross-repository workflow semantics or dependency gates. These are a deliberate
  follow-on: `plans/cross-repo-workflows/` builds on this fleet substrate once it
  lands. Phase 3 carries one forward-compatibility constraint for that plan (keep
  "which repos do I scan" separable from "which repos may I write to"); see
  `prompts/phase-3-production-config.md`.
- Per-repository workflow definitions. All repos use the compiled reference
  workflow for now.
- Making hints authoritative. Workers still re-scan Forge state; hints only wake
  the shared worker pool.
- Changing the `Forge` trait unless a phase proves an unavoidable portable gap.

## Design constraints

- Keep the existing single-repo worker primitives usable; add a multi-repo layer
  that delegates to the proven per-repo tick path.
- Repository identity must be explicit in reports, logs, errors, and tests.
- A failure in one repository should not prevent scanning the remaining repos in
  the same tick; surface partial failure clearly.
- Labels are still per repository. Add or reuse an idempotent "ensure workflow
  labels for this repo" path before workers are expected to process a repo.
- Role identity is still one Forge token per worker process. That token must have
  permission on every repo assigned to that worker.
- Integrate with the completed hint/wake path after `hint-driven-wakeups` lands:
  repo-specific hints may narrow the next scan, but a broad/full scan remains
  the correctness backstop.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Memory backend unit tests + multi-repo runner core.**
   `prompts/phase-1-memory-runner-core.md`

   Add a backend-agnostic multi-repo runner layer, with fast unit tests on
   `MemoryForge`. Prove one role worker and one mechanical worker can process
   two repositories independently, with no cross-repo label/comment/PR leakage.
   Done: `harness-runner::RepositorySet`/`RepositoryTarget` define the ordered
   repository set and hint-prioritized scan helpers; `MultiRepoRoleWorker` and
   `MultiRepoMechanicalWorker` delegate to the existing single-repo workers;
   `MultiRepoTickReport`/`MultiRepoError` report repository-scoped partial
   failures while continuing remaining repos.

2. ☑ **Phase 2 — Filesystem backend integration tests.**
   `prompts/phase-2-filesystem-integration.md`

   Exercise the same multi-repo layer on `FilesystemForge` with distinct handles
   and process-style isolation. Prove repository-scoped persistence, leases, and
   recovery stay independent across repos. Done: `harness-runner` now has
   `tests/multi_repo_filesystem.rs`, covering durable two-repo role progress
   across fresh handles, per-repo mechanical journals with identical command ids,
   repo-local lease/dependency recovery after wrapper restart, and filesystem
   hint wakeups plus missed-hint polling backstop. No repo-keying fixes were
   needed; the invariant is one `RepositoryId` plus one repository-bound journal
   per target.

3. ☑ **Phase 3 — Production CLI/config/provisioning.**
   `prompts/phase-3-production-config.md`

   Extend production worker/provisioning surfaces from one `--repo` to an
   explicit repository set, while keeping the old single-repo mode as a special
   case. Ensure labels can be applied idempotently to every configured repo.
   Done: `harness-worker` accepts repeated `--repo owner/name` values and
   `--repo-list <path>` (one `owner/name` per non-comment line), deduplicates the
   configured scan shard, resolves every repo at startup with redacted
   not-found/not-readable errors, logs the resolved set, ensures workflow labels
   in every configured repo, and constructs production `MultiRepoRoleWorker` /
   `MultiRepoMechanicalWorker` instances. Wake datagrams now carry the parsed
   `ChangeHint`; role workers prioritize known hinted repos and log unknown-repo
   hints as broad scans. The example config keeps `OWNER`/`NAME` as the legacy
   single-repo default and adds optional `REPOS="owner/a owner/b"`; worker token
   Forge permissions, not scan-shard membership, remain the write authority.

4. ☑ **Phase 4 — Multi-repo e2e tests.**
   `prompts/phase-4-e2e.md`

   Add end-to-end regressions that start one fixed worker set and file issues in
   multiple repos. Include a deterministic filesystem/process test and a gated
   real-Forgejo test using the completed webhook wake path. Done:
   `harness-testing/tests/multi_repo_multiprocess.rs` provisions two filesystem
   repos and runs one fake worker set over both; `forgejo_multi_repo_webhook.rs`
   boots throwaway Forgejo + real runner, provisions a second repo, registers
   webhooks for both repos, launches one multi-repo fake worker set with long
   polling, and requires convergence before the poll backstop.

5. ☐ **Phase 5 — Examples and operator docs.**
   `prompts/phase-5-examples-and-docs.md`

   Update `examples/reference-delivery/` so it demonstrates one worker pool over
   multiple repos, documents how to configure the repo set, and validates that a
   new issue in any configured repo is picked up promptly.

## Acceptance criteria

- Memory unit tests prove multi-repo role and mechanical behavior without a real
  filesystem or network.
- Filesystem integration tests prove independent repo processing with separate
  handles and durable state.
- Production workers accept a configured repository set and still support the
  existing single-repo invocation.
- One fixed set of workers processes issues filed in at least two repos in e2e.
- Webhook/hint integration wakes the shared worker pool after events from any
  configured repo, while polling remains a liveness backstop.
- `examples/reference-delivery` documents and demonstrates the multi-repo mode.
- `cargo fmt --all`, `cargo dev-clippy`, and `cargo dev-check` pass at each
  phase; default tests remain hermetic.

## Relevant starting points

- `plans/hint-driven-wakeups/README.md` — must be complete before this starts.
- `crates/harness-runner/src/worker.rs`
- `crates/harness-runner/src/scan.rs`
- `crates/harness-runner/src/driver.rs`
- `crates/harness-runner/src/trigger.rs`
- `crates/harness-production/src/worker.rs`
- `crates/harness-production/src/worker_args.rs`
- `crates/harness-production/src/provision.rs`
- `crates/harness-testing/src/worker_bin/`
- `crates/harness-testing/tests/multiprocess.rs`
- `crates/harness-testing/tests/forgejo_multiprocess.rs`
- `examples/reference-delivery/run.sh`
