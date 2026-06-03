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
| [0003](0003-use-temper-workflow-for-workflow-layer.md) | Use `temper-workflow` for the workflow layer | architecture, crates, workflow |
| [0004](0004-prefer-native-forge-state-over-mirror-labels.md) | Prefer native Forge state over mirror labels | architecture, forge, workflow, ci |
| [0005](0005-avoid-redundant-pending-in-queue-labels.md) | Avoid redundant pending in queue labels | workflow, naming, labels |
| [0006](0006-wire-modules-and-run-tests-not-just-build.md) | Wire new modules into the crate and run tests, not just build | rust, tooling, forgejo, process |
| [0007](0007-forgejo-cli-token-and-runner-gotchas.md) | Forgejo 7.0.x CLI token + runner registration gotchas | forgejo, ci, testing, tooling |
| [0009](0009-cap-throwaway-forgejo-cpu.md) | Cap (and clean up) the throwaway Forgejo's CPU in the e2e | forgejo, ci, testing, tooling, process |
| [0011](0011-validate-blocking-launch-script.md) | Validate the blocking launch script in the background, then stop via its sentinel | process, tooling, forgejo, testing |
| [0013](0013-ci-workflow-string-literal-strips-yaml-indentation.md) | Don't build indented YAML with `\`-continued string literals | forgejo, ci, rust, provisioning, temper-production |
| [0014](0014-allow-loopback-for-throwaway-forgejo-webhooks.md) | Allow loopback for throwaway Forgejo webhooks | forgejo, webhook, testing, configuration |
| [0015](0015-start-downstream-wake-sockets-before-seeding-work.md) | Start downstream wake sockets before seeded work can hand off | webhook, process, forgejo, testing |
| [0016](0016-refresh-demo-binaries-before-launch.md) | Refresh demo binaries before launch | examples, tooling, process, forgejo |
| [0017](0017-cross-repo-demo-needs-closing-architect.md) | Cross-repo demo needs the closing architect | examples, cross-repo, workflow, agents, forgejo |
| [0018](0018-snapshot-long-running-shell-launchers.md) | Snapshot long-running shell launchers | examples, process, shell, teardown |
| [0019](0019-demo-ci-verdicts-follow-github-sha.md) | Demo CI verdicts follow `GITHUB_SHA` | examples, forgejo, ci, testing |
| [0020](0020-dogfood-prs-must-not-be-bookkeeping-only.md) | Keep dogfood identities and PR diffs honest | dogfood, agents, forgejo, product-chat |
| [0021](0021-user-defined-roles-own-prompt-behavior.md) | Keep workflow-role prompts user-defined | agents, workflow, prompts, architecture |
| [0022](0022-forgejo-review-merge-timestamps-can-tie.md) | Treat equal Forgejo review and merge timestamps as ordered by state | forgejo, testing, reviews, ci |
| [0023](0023-keep-agents-md-as-orientation-map.md) | Keep `AGENTS.md` as an orientation map | docs, process, agents |
| [0024](0024-product-manager-is-an-interactive-profile.md) | Treat product-manager as an interactive profile | architecture, agents, workflow, product-chat, interaction |
| [0025](0025-process-boundary-for-interactive-responders.md) | Prefer process boundaries for interactive responders | architecture, agents, interaction, process-boundary, pi-sdk |
