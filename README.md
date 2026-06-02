# Harness — issue-tracker-native orchestration for AI software delivery

Harness is a Rust workflow runtime that lets AI agents and humans deliver software through the same source-control issue tracker. It turns Forgejo-style issues, pull requests, labels, comments, dependency links, reviews, CI jobs, and merges into a durable state machine: workers get role-scoped queues and tools, every mutation is checked against the declared workflow, and the Forge remains the shared source of truth.

[Docs](docs/README.md) · [Agentic workflows](docs/explanation/agentic-workflows.md) · [Workflow contract](docs/reference/workflow-layer.md) · [Forge interface](docs/reference/forge-interface.md) · [Reference demo](examples/reference-delivery/README.md)

## The idea

AI coding is easier to trust when it behaves like a teammate, not like a hidden automation script. Harness keeps autonomous work inside normal project artifacts:

- an issue describes the work;
- labels and metadata classify its workflow state;
- a role claims it with a lease;
- a pull request carries the implementation;
- CI and native review decisions are gates;
- merges, dependency unblocks, and recovery happen only when the workflow allows them.

Humans can open the Forge UI and see what happened. Agents can restart, repeat a tool call, or lose context without owning durable truth outside the tracker.

## What Harness does

Harness watches configured repositories and repeatedly asks:

- Which issues or pull requests match an active workflow queue?
- Which role is allowed to act on them?
- Which transition is legal from the current labels, metadata, dependencies, CI, and review state?
- Is the next step judgment work for an agent, or mechanical controller work?
- Did a crash, expired lease, partial transition, or cross-repo dependency require repair?

The answers come from a validated workflow spec and fresh Forge state. Agents do not receive generic permission to mutate labels or merge PRs; they receive narrow workflow tools derived from the transitions their role is authorized to run.

## A two-minute example

A human files:

```text
Add password reset
```

A reference workflow can move it like this:

```text
architect  -> turns intake into ready engineering work
engineer   -> claims the code issue and opens an implementation PR
CI         -> reports pass/fail on the PR head
reviewer   -> approves or requests changes through native PR review
owner      -> merges only after CI and review gates are both green
controller -> repairs partial work, expires stale leases, and unblocks dependencies
```

If CI fails or review requests changes, the PR routes back to the engineer. If a prerequisite issue lands, dependency resolution can unblock the next issue mechanically. If one intake fans out across repositories, the parent stays blocked until every child work item lands in its own repository.

## Architecture at a glance

```text
Forge plane
  issues · PRs · labels · comments · dependencies · reviews · CI · merges
  + harness metadata blocks in artifact bodies
        ↑
harness-forge
  provider-neutral Forge trait and domain model
        ↑
harness-workflow
  validate specs · classify artifacts · evaluate queues/gates · plan/execute
  transitions · enforce idempotency · manage leases · reconcile drift
        ↑
harness-runner
  scan repositories · dispatch role workers · run mechanical workers · bind
  external tools such as coding workspaces
        ↑
harness-agents / harness-production
  real LLM role agents, Forgejo wiring, provisioners, webhook trigger, workers
```

The key invariant is simple: **the Forge is authoritative**. Polling is the correctness backstop; webhooks are only wake-up hints. Every transition reloads current state before mutating.

## Current shape

Harness is an active Rust workspace, not just a README sketch. The core workflow runtime is implemented and tested against reference backends; Forgejo support, production worker wiring, LLM agents, webhook wakeups, cross-repo workflow modeling, and reference-delivery rehearsals live in this repository. The operator demo is useful for topology and behavior, but read its README for current caveats before treating it as a turnkey deployment.

## Start here

- New to the concepts: read [`docs/explanation/agentic-workflows.md`](docs/explanation/agentic-workflows.md).
- Need exact runtime behavior: read [`docs/reference/workflow-layer.md`](docs/reference/workflow-layer.md).
- Need the backend contract: read [`docs/reference/forge-interface.md`](docs/reference/forge-interface.md).
- Want the reference workflow: read [`docs/explanation/reference-workflow.md`](docs/explanation/reference-workflow.md).
- Want to run deterministic scenarios: `cargo test -p harness-runner --test end_to_end`.
- Coding agent starting a session: read [`AGENTS.md`](AGENTS.md) after this file.
