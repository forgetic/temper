# Smith

**LLM workers for Temper.**

*Agents that perform work inside Temper workflows.*

Smith provides the first concrete agent implementations for Temper.

Temper owns workflow execution, state transitions, permissions, and interaction contracts. Smith owns the agents that reason, make decisions, write code, and produce responses within those contracts.

Smith does not own workflow state.

Smith does not mutate Forge artifacts directly.

Smith receives structured work from Temper, produces structured results, and returns control to the workflow engine.

[Docs](docs/README.md) · [Provider auth](docs/how-to/configure-provider-auth.md) · [Run responders](docs/how-to/run-temper-responders.md) · [Observability](docs/reference/workflow-role-observability.md) · [Process boundary](docs/explanation/process-boundary.md)

---

## Why

Temper treats workflows as durable state machines whose state lives in the Forge.

Agents are intentionally separate.

A workflow engine should be able to:

* validate transitions
* enforce permissions
* recover from failures
* reconcile workflow state
* survive agent crashes

without depending on any particular LLM implementation.

Smith exists to provide those implementations.

It is the home for provider integrations, authentication, prompting, decision-making, coding agents, and other forms of LLM-driven work.

Temper decides what work is allowed.

Smith decides how to perform it.

---

## Core ideas

### Temper owns workflow execution

Temper is authoritative for:

* workflow definitions
* queues
* leases
* transitions
* permissions
* workflow state
* Forge mutations

Smith is authoritative for none of those things.

A Smith agent cannot advance a workflow on its own.

All workflow changes must pass through Temper validation.

### Smith owns agent execution

Smith is responsible for:

* provider integrations
* authentication
* model selection
* prompts
* reasoning
* structured decisions
* coding agents
* responder implementations

Smith turns workflow tasks into agent behavior.

### Process boundaries are intentional

Temper and Smith communicate through stable process protocols.

This separation keeps workflow correctness independent from agent implementation details.

Either side can evolve independently.

Different agent implementations can be added without changing workflow semantics.

Different workflow engines can reuse Smith agents without inheriting Temper internals.

### Agents are replaceable

Smith provides the first production-quality agents for Temper.

They are not the only possible agents.

Organizations may:

* replace Smith entirely
* implement alternative responders
* mix Smith agents with custom agents
* combine agents and human-operated roles

The workflow contract remains the same.

---

## What Smith does

Given a workflow task from Temper, Smith may:

* analyze an issue
* classify work
* make a structured workflow decision
* write code
* prepare a change proposal
* generate a review
* answer an interactive request
* invoke a coding agent

The result is returned as structured output for Temper to validate and apply.

Smith performs work.

Temper decides whether that work is accepted.

---

## Architecture

```text
Forge
  authoritative workflow state
        ↑
Temper
  workflow execution engine
  queues · transitions · permissions · recovery
        ↑
process protocols
        ↑
Smith
  providers · authentication · prompts
  decisions · coding agents · responders
        ↑
LLMs
  OpenAI · Anthropic · DeepSeek · others
```

The key invariant is simple:

> Smith can think. Temper decides.

---

## What Smith owns today

The current implementation includes:

* OpenAI/ChatGPT authentication and provider integrations
* Anthropic authentication and provider integrations
* DeepSeek integrations
* structured workflow-role decision responders
* interactive profile responders
* coding-agent integrations through `pi_agent_rust`
* provider validation and live integration tests

These implementations serve as the default agent layer for Temper deployments.

---

## Relationship to Temper

Temper and Smith are designed to evolve independently.

Temper remains responsible for:

* workflow definitions
* workflow execution
* workflow correctness
* Forge integration
* state reconciliation

Smith remains responsible for:

* agent implementations
* model integrations
* reasoning behavior
* coding-agent execution
* interactive responders

Together they provide a complete system for running agentic workflows on top of a Forge.

Separately they maintain a clean boundary between workflow correctness and agent autonomy.
