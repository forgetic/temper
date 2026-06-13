# ADR 0023: Multi-repo co-development jobs

## Status

Accepted

## Context

Temper already handles cross-repo work by **decomposition**: the architect
triages a high-level intake issue and fans it out into a parent epic plus
independent child code issues, which may live in different repositories
(`fan_out_architect_children`, ADR 0011/0015/0021). Each child is then an
ordinary single-repo job — its own assignment, its own one-repo workspace, its
own pull request — and the children are tied together only by declarative
dependency links.

That shape is wrong for a whole class of changes: the ones that must be
**written and validated together** because the repositories are source-coupled.
In this tree:

- `smith/crates/smith-worker` depends on `temper/crates/temper-worker-protocol`
  by a **path** dependency (`../../../temper/...`).
- `temper` depends on `skein` by a **path** dependency
  (`../skein/crates/skein`).

There is no version or git pin between these crates — the only thing that
resolves them is a **sibling checkout layout**. A change to the worker protocol
in `temper` and the consuming code in `smith` cannot be compiled, let alone
tested, in two separate single-repo checkouts. Decomposing it into independent
children that each see only their own repo is a category error: neither child
builds alone.

The whole worker pipeline today assumes *one repo per artifact, per workspace,
per assignment, per PR*: `Capability { role, repo }`, an `Assign` naming one
`repo`, a `JobContext` with one `repository`/`branch_hint`, a coding workspace
rooted at a single git worktree, and a result carrying a single `branch`. So
the co-development case cannot be expressed at all.

## Decision

Add a second, complementary mode — **the coordinated job** — in which one
engineer assignment assembles a *workspace of several repositories*, edits any
subset of them in one agent turn, and opens **one pull request per changed
repository**. Decomposition (independent children) stays; the architect chooses
per issue. The defining property of the coordinated mode is co-development: the
cross-cutting change is authored and validated as a unit.

This is a **clean-slate, breaking** change to the worker/daemon wire protocol
and the worker↔agent protocol. We are pre-deployment; there is no
back-compatibility obligation, and we explicitly reject shoehorning multi-repo
semantics into the single-repo fields. The protocol stays `v1`
(`WORKER_PROTOCOL_VERSION` unchanged): while we are pre-1.0 alpha we revise the
wire shapes in place rather than bumping the version number.

### A. The workspace manifest (job input)

A coding job's checkout target generalizes from one repository to an ordered
`WorkspaceManifest`:

```jsonc
{
  "coordination_key": "coord-for-code-42",   // stable id for the PR set
  "repos": [
    { "repo": "ai/temper", "dir": "temper", "access": "writable",
      "default_branch": "main", "base_branch": "main",
      "branch_hint": "agent/coord-for-code-42" },
    { "repo": "ai/smith",  "dir": "smith",  "access": "writable",
      "default_branch": "main", "base_branch": "main",
      "branch_hint": "agent/coord-for-code-42" },
    { "repo": "ai/skein",  "dir": "skein",  "access": "read_only",
      "default_branch": "main", "base_branch": "main" }
  ]
}
```

- The **first** repo is the *primary* — the home of the coordinating issue. The
  job's lease, progress relay, and source-issue resolution all key off the
  primary, exactly as today.
- `dir` is the relative path under the workspace root where the repo is checked
  out. **The manifest is responsible for laying repositories out so their
  inter-repo path dependencies resolve.** For this tree that means flat
  siblings named `temper/`, `smith/`, `skein/`, which is exactly what
  `../skein/crates/skein` and `../../../temper/...` expect. The combined build
  then works with **no `[patch]` rewriting** — the directory layout *is* the
  dependency resolution.
- `access` is `writable` (eligible for a commit/push/PR) or `read_only` (present
  only so the build resolves; never pushed).

A single-repo job is just the degenerate manifest of one writable primary;
there is no separate single-repo code path.

### B. Capability semantics

`Capability { role, repo }` is unchanged. A coordinated job's work item carries
the full repo set, and dispatch requires the chosen worker to hold
`(role, repo)` for **every** repository in the manifest — writable and
read-only alike. This is the simplest correct rule: the operator registers a
worker for precisely the repos it can check out (and, for writable repos, push
to). No new capability kind, no read-only/write-only split.

### C. The job result (job output)

A successful coordinated job reports `repos: Vec<RepoOutcome>` — one entry per
**writable repository that produced a diff** — each carrying the repo and the
pushed `branch`. Writable repos the agent left untouched produce no outcome and
no PR. The daemon opens one PR per outcome.

### D. The pull-request set and its linkage

On success the daemon opens one PR per `RepoOutcome`. Every PR in the set:

- targets its own repository on its `agent/<coordination_key>` branch;
- carries workflow metadata linking it to the coordinating issue as a
  repo-qualified `ArtifactRef` (ADR 0021), and stamps the shared
  `coordination_key` so the whole set is discoverable from any member.

### E. Workspace assembly (worker)

The worker checks out each manifest repo into `<root>/<dir>`: writable repos
onto their work branch (fetch-or-fresh, as today), read-only repos at their
pinned base ref. It then runs **one** agent turn with the working directory set
to the **workspace root** — not a single repo — so the agent can read and build
all repos together. On completion, for each *writable* repo it detects a diff or
commits-ahead-of-base, commits, pushes the work branch, and records a
`RepoOutcome`. Read-only repos are never committed or pushed.

### F. The worker↔agent protocol

`WorkspaceContext` carries the workspace root and the repo list (each with its
`dir` and `access`) instead of a single `repository`. The agent edits across
dirs and returns one `WorkspaceResult` (verdict/summary/body) for the whole
turn. Per-repo diffs are **discovered by the worker**, not declared by the
agent — the agent never has to enumerate which repos it touched.

## Consequences

- The engineer can address a genuinely cross-cutting issue (e.g. a worker-
  protocol change spanning `temper` + `smith`) from a single issue, in a single
  workspace, with a coherent multi-PR product — the co-development case the
  decomposition path could not serve.
- Combined builds need no dependency rewriting: the manifest's sibling layout
  satisfies the existing path deps directly. This is load-bearing and must be
  preserved by whoever constructs a manifest (architect/daemon).
- Authority boundaries are unchanged (ADR 0002): the agent still never holds a
  Forge token; the worker pushes branches; the daemon opens the PRs.
- The lease/progress/source-issue machinery is unchanged because it keys off the
  primary (coordinating) artifact, which remains single.

## Out of scope (this phase)

- **Coordinated/serial landing and cross-repo CI ordering.** The PRs are opened
  independently; how a dependent PR's CI is gated on a prerequisite landing is a
  separate decision deliberately deferred. (Because the inter-repo deps are
  path-based, combined-checkout CI — the same sibling layout the engineer used —
  is the natural primitive when we get there, with serial landing as an
  optimization for acyclic additive changes.)
- **The architect's heuristic** for choosing coordinated vs. decomposition. The
  mechanism is built here; the policy that selects it is left to the role prose.

## Alternatives considered

- **Reuse the epic + children decomposition.** Rejected: the children don't
  build in isolation (path-coupled crates), and the user requirement is a single
  coordinating issue, not a fan-out.
- **Keep one-repo jobs and add a `[patch]`-rewriting step so a single-repo
  checkout can pull siblings in.** Rejected: it reintroduces dependency
  rewriting the path layout already avoids, and still gives the agent only one
  repo to edit.
- **An optional `extra_repos` field bolted onto the single-repo job.** Rejected
  as shoehorning. Generalizing the checkout target to a manifest (single-repo as
  the degenerate case) is cleaner and removes the special case rather than
  adding one.
</content>
</invoke>
