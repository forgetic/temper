# Lesson 0004: Prefer native Forge state over mirror labels

## Tags

`architecture`, `forge`, `workflow`, `ci`

## Trigger

Human steering questioned the `testing-passed` and `testing-failed` labels
because current CI status is already observable from GitHub and Forgejo.

## What went wrong

The workflow kept labels and tester transitions that mirrored CI outcomes after
native `CiJob`-based gates existed. That duplicated Forge-owned truth and could
drift from new commits or rerun CI results.

## Steering for future agents

Before adding a workflow label for provider-owned facts, check whether the
portable Forge model already exposes that fact. Prefer runtime gate/queue
signals over mirror labels; extend the portable model only when the fact is not
already represented.

## Where this is now documented

- `docs/adr/0017-retire-testing-labels-for-native-ci-status.md`
- `docs/reference/workflow-layer.md`
- `docs/explanation/reference-workflow.md`
