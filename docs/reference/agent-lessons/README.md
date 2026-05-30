# Agent lessons register

This register captures recurring mistakes, failed assumptions, and human steering that future agents should learn from.

Use it as a compact memory layer, not as a replacement for current docs. If a lesson becomes a durable rule, also update the relevant README, how-to guide, reference page, explanation, or ADR.

## When to read

At session start, read this index after `docs/README.md`. Then open entries whose tags match the task.

For broad architecture or crate-boundary work, read all active lessons.

## When to add or update a lesson

Add or update a lesson when:

- a human corrects an agent's assumption or design direction
- an agent makes a mistake that could recur
- validation fails because a documented workflow was missing or misleading
- a workaround or steering rule would save future context or review time

Do not add a lesson for ordinary implementation details that are already captured in code, tests, or reference docs.

## Entry format

Use `template.md`. Keep each entry short and specific.

## Active lessons

| ID | Title | Tags |
| --- | --- | --- |
| [0001](0001-keep-forge-abstractions-out-of-core.md) | Keep Forge abstractions out of the workflow crate | architecture, crates, forge |
| [0002](0002-keep-source-files-under-600-lines.md) | Keep source files under 600 lines | workflow, rust, maintainability |
| [0003](0003-use-harness-workflow-for-workflow-layer.md) | Use `harness-workflow` for the workflow layer | architecture, crates, workflow |
| [0004](0004-prefer-native-forge-state-over-mirror-labels.md) | Prefer native Forge state over mirror labels | architecture, forge, workflow, ci |
| [0005](0005-avoid-redundant-pending-in-queue-labels.md) | Avoid redundant pending in queue labels | workflow, naming, labels |
| [0006](0006-wire-modules-and-run-tests-not-just-build.md) | Wire new modules into the crate and run tests, not just build | rust, tooling, forgejo, process |
| [0007](0007-forgejo-cli-token-and-runner-gotchas.md) | Forgejo 7.0.x CLI token + runner registration gotchas | forgejo, ci, testing, tooling |
