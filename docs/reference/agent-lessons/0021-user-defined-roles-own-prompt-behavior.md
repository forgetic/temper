# Lesson 0021: Keep workflow-role prompts user-defined

## Tags

`agents`, `workflow`, `prompts`, `architecture`

## Trigger

A dogfood investigation found that `harness-agents` shipped checked-in
engineer/architect/reviewer/owner/human prompts and role-specific adapters on the
production worker path. A human clarified that workflow roles are user-defined,
so production code should not carry hard-coded role behavior.

## What went wrong

It was tempting to treat the existing reference-delivery prompts as production
presets and merely move or namespace them. That still bakes reference workflow
judgment into the product and bypasses the intended workflow compilation model.

## Steering for future agents

Generated prompts should contain only workflow mechanics: role id, queues,
authorized actions, output format, and authority boundaries. Role judgment,
engineering guidance, and tool-use instructions belong in user workflow config
or test/demo fixtures. Non-workflow tools must be explicitly declared by the user
and bound by the runner before an LLM can use them.

## Where this is now documented

- `plans/user-defined-role-agents/README.md`
- `docs/reference/agent-lessons/0020-dogfood-prs-must-not-be-bookkeeping-only.md`
