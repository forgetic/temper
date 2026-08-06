# Temper

**Issue-tracker-native execution engine for agentic workflows.**

*Run agentic workflows on top of your Forge.*

Temper is a workflow runtime that executes agentic workflows using your Forge as the source of truth.

Issues, pull requests, labels, reviews, CI status, comments, and dependency links become workflow state. Temper evaluates that state against a declared workflow and dispatches work to agents while enforcing valid transitions.

The Forge remains authoritative. There is no separate workflow database, no hidden workflow state, and no requirement to adopt a specific delivery process.

Define a workflow as a state machine. Temper executes it.

[Docs](docs/README.md) · [Agentic workflows](docs/explanation/agentic-workflows.md) · [Workflow contract](docs/reference/workflow-layer.md) · [Forge interface](docs/reference/forge-interface.md) · [Reference demo](examples/reference-delivery/README.md)

---

## Operator command flow

The public operator lifecycle is explicit:
`temper init -> temper check -> temper plan -> temper apply -> temper serve`.

```sh
temper --config ./deploy init        # write a local deployment bundle
temper --config ./deploy check       # validate config, credentials, pools, secrets
temper --config ./deploy plan        # preview Forge reconciliation without mutation
temper --config ./deploy apply       # provision the Forgejo repo/users/labels/webhook
temper --config ./deploy serve standalone
# or split runtime services:
temper --config ./deploy serve engine
temper --config ./deploy serve worker --pool engineers
```

`temper init --apply` combines the local write and provisioning steps for demos.
For production, review the generated config/workflow, run the offline checks,
and inspect `temper plan` before `temper apply`; `temper check --component engine`
and `temper check --component worker --pool <name>` validate the same bundle
without starting services. Use top-level `temper check` for validation —
`temper config validate` remains only a compatibility path.

`temper serve` is the documented long-lived runtime surface. Legacy
`temper daemon` forms are still accepted for existing automation but are hidden
from onboarding help. The engine owns the Forgejo webhook endpoint
(`/forgejo/webhook`); webhook deliveries are wake hints and polling remains the
correctness backstop, so there is no separate trigger service.

Host recovery uses the separate, dry-run-first `temper maintenance` surface.
Codebase-memory cleanup and stable-project rebuilds are documented in
[Recover a codebase-memory cache safely](docs/how-to/recover-codebase-memory.md);
provider cache-directory deletion is not a supported procedure.

Temper's long-term environment surface is intentionally narrow: standard
`HOME`/`XDG_*`, systemd `CREDENTIALS_DIRECTORY`, logging controls, and the
single worker-to-agent provider credential env. Put deployment state in
config/credentials files or systemd credentials, not `TEMPER_*` overrides. See
[Environment variables](docs/reference/environment-variables.md) and the
[systemd examples](examples/systemd/README.md).

---

## Why

Most workflow systems keep their state in a separate orchestration service.

Issues are inputs. Pull requests are outputs. The actual workflow lives somewhere else.

Temper takes the opposite approach.

The Forge already contains the artifacts that software teams use to coordinate work:

* issues
* pull requests
* labels
* reviews
* CI results
* dependency links
* comments

Temper treats those artifacts as workflow state.

A workflow is defined as a state machine over Forge artifacts. Workers inspect Forge state, perform authorized actions, and advance work through declared transitions. If an agent crashes, loses context, or restarts, workflow state remains intact because it lives in the Forge itself.

Humans and agents participate through the same interface and observe the same source of truth.

---

## Core ideas

### The Forge is authoritative

Temper does not maintain a separate workflow database.

Current workflow state is derived from the Forge itself:

* labels
* metadata
* dependencies
* reviews
* CI status
* pull requests
* merge state

Polling is the correctness backstop. Webhooks are wake-up hints.

Every transition reloads current Forge state before mutating anything.

### Workflows are state machines

Temper does not prescribe a software delivery process.

Users define workflows as state machines with:

* roles
* queues
* transitions
* gates
* dependency rules
* recovery behavior

Temper validates and executes those workflows.

Whether the process is simple or complex is entirely up to the workflow definition.

### Agents operate through workflow contracts

Agents do not receive broad permission to manipulate repositories.

Instead, Temper derives role-specific capabilities from the workflow definition.

An agent can only perform actions that are valid for:

* its role
* the current state
* the current transition

This keeps autonomous behavior aligned with declared workflow rules.

### Durable execution

Workflow state survives:

* agent crashes
* process restarts
* context loss
* partial transitions
* expired leases
* infrastructure failures

Because the durable state already exists in the Forge.

Temper continuously reconciles observed state against workflow expectations and repairs drift when possible.

---

## What Temper does

Given a set of repositories and workflow definitions, Temper repeatedly asks:

* Which artifacts match an active queue?
* Which role is eligible to act on them?
* Which transitions are legal from the current state?
* Which work requires agent judgment?
* Which work can be handled mechanically?
* Has anything stalled, expired, or drifted from the workflow?

The answers come from the workflow specification and current Forge state.

Temper then dispatches work, executes transitions, and keeps the workflow moving forward.

---

## A simple example

A team defines a workflow with the following roles:

These role names are illustrative examples for this workflow; Temper does not require or hard-code them.

```text
architect
engineer
reviewer
owner
controller
```

A new issue is created:

```text
Add password reset
```

The workflow might define transitions like:

```text
architect  -> prepare implementation work
engineer   -> implement and open PR
reviewer   -> approve or request changes
mechanical -> merge after required gates pass
owner      -> review post-merge alignment
controller -> repair, recover, and unblock work
```

Temper does not hard-code this process.

It simply evaluates the workflow definition, observes Forge state, and executes valid transitions.

If review requests changes, the workflow may route back to engineering.

If CI fails, the workflow may block progress.

If dependencies are resolved, the workflow may automatically unblock waiting work.

The behavior comes from the workflow definition, not from Temper itself.

---

## Architecture

```text
Forge plane
  issues · PRs · labels · comments · dependencies · reviews · CI · merges
  + Temper metadata blocks in artifact bodies
        ↑
temper-forge
  provider-neutral Forge trait and domain model
        ↑
temper-workflow
  workflow validation
  queue evaluation
  transition execution
  gate enforcement
  lease management
  drift reconciliation
        ↑
temper-runner
  repository scanning
  role dispatch
  mechanical workers
  external tool integration
        ↑
focused runtime crates + external responder processes
  Forgejo worker wiring
  provisioning and Forgejo ops
  wake sockets and webhook triggers
  interaction service transports
  process-bound role/chat responders
```

The key invariant is simple:

> The Forge is the source of truth.

Everything else can be restarted, replaced, or recovered from current Forge state.

---

## What Temper is not

Temper is not:

* an issue tracker
* a project management tool
* a coding agent
* a workflow definition language tied to a single process
* a hidden automation layer that owns workflow state

Temper is an execution engine.

It runs agentic workflows whose durable state lives in the Forge.

---

## Status

Temper is under active development.

The current implementation is opinionated toward agentic software-delivery workflows and runs role-specific behavior through explicit process responder boundaries. The underlying workflow engine is designed around a more general model: state-machine workflows executed against Forge artifacts as the authoritative source of truth.
