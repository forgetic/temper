# Cross-repository workflow model

Cross-repository workflow support lets one intake issue become coordinated work
across several repositories while keeping execution local to each repository.
The model builds on the fixed multi-repo worker pool: every role process scans a
configured repository set, but each child issue and pull request is still handled
by the normal per-repo workflow.

## Shape of the flow

1. A human files one intake issue in a source repository.
2. The architect decides whether the request is ordinary same-repo work or a
   fan-out plan.
3. For fan-out, the architect creates one child code issue per repository-scoped
   piece of work. Each child names its target repository explicitly.
4. Engineers, reviewers, CI, and owners service each child in that child's repo;
   they do not coordinate through shared branches or cross-repo transactions.
5. The parent intake issue remains blocked until dependency aggregation observes
   every child as landed from fresh Forge state.

This keeps the risky part small: only planning, references, and aggregation know
about cross-repo links. The delivery roles keep the same queues and tools they
use for single-repo work.

## Repo-qualified references

Workflow links use a repo-qualified artifact reference: a repository id plus an
item number. Same-repository links can still use the old bare item-number
shorthand, but cross-repo links carry the explicit repository id. This is the
portable model recorded in [ADR 0021](../adr/0021-repo-qualified-artifact-references.md).

The reference is not a Forgejo URL. URLs are useful for humans, but the workflow
needs a provider-neutral identity it can resolve through the `Forge` trait.

## Parent aggregation

The parent records dependency links to every planned child. During scans and
reconciliation the runtime reads each target from its own repository and reduces
those reads to the planner's simple dependency signal: landed or not landed. If a
child repository cannot be read, that child is treated as not landed for the
current scan, so the parent never unblocks from stale or incomplete information.

There is no atomic cross-repo merge. Children may land in any order. The parent
is only an aggregation record that resolves after all required child work has
reached its terminal state.

## Operational implications

A cross-repo intake can only succeed when the worker identities have Forge
permission on every involved repository. The scan shard decides where workers
look for work; Forge permissions decide where they may create child issues and
later mutate workflow state. Labels, CI, and webhooks are still provisioned per
repository.

For the exact correlation-key and relation contracts, see the
[cross-repo workflow reference](../reference/cross-repo-workflows.md). For the
operator demo, see the
[cross-repo reference-delivery recipe](../how-to/run-cross-repo-reference-delivery-demo.md).
