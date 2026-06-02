# ADR 0010: Model external-signal gates as Forge-projected conditions

## Status

Accepted; CI-specific projection superseded by ADR 0014

## Context

Workflow gates originally had one satisfaction path: a gate named the
transitions whose visible label effects proved the gate was complete. That works
for role actions such as review approval and test pass, where the workflow owns
the transition.

Some gates are driven by systems outside the role workflow. CI is the motivating
case in the reference delivery workflow: a backend or runner adapter observes
the Forge CI result and projects it into portable workflow state (`ci = passed`,
backed by the `ci-passed` label). No agent role should run a fake CI transition,
and `temper-forge` must not grow provider-specific CI-gate semantics.

The fixture previously used zero-role adapter transitions
(`record_ci_pass`/`record_ci_failure`) only so `ci_gate` could name a
`satisfied_by` transition. That made the gate plan, but it encoded an external
signal as an uncallable workflow action and made zero-role transitions
ambiguous.

## Decision

Add an external-signal gate condition to the workflow spec. A gate may still use
`satisfied_by` transitions, and may also declare one portable `condition`:

- `label_present { label }` — satisfied when the classified artifact carries the
  declared label.
- `state_equals { dimension, state }` — satisfied when the classified artifact
  occupies the declared state in that dimension.

The condition is evaluated on the same classified artifact as the guarded
transition. For CI, adapters originally remained outside `temper-forge`: they observed the
provider-specific CI result and projected it into labels/state that the workflow
already knew how to classify. ADR 0014 supersedes that CI-specific projection by
adding a native `ci_passed` gate condition fed from portable `CiJob` conclusions;
`label_present` and `state_equals` remain valid for other projected signals.

A required gate is satisfied when either its external condition holds or one of
its `satisfied_by` transition outcomes is visible. Existing transition-satisfied
gates therefore keep their behavior unchanged.

## Consequences

- CI and similar signals are modeled as portable workflow state instead of
  provider-specific concepts or role prose.
- The reference delivery fixture could express `ci_gate` directly over
  `ci = passed` and remove the zero-role adapter transitions. ADR 0014 later
  replaced that CI projection with the native `ci_passed` condition.
- Label manifests can identify labels that exist because they are external gate
  conditions, even when no workflow transition writes them.
- External adapters must be idempotent projectors of Forge-observed state; the
  workflow layer consumes only their projected labels/states.

## Alternatives considered

Keep the zero-role adapter-transition workaround. Rejected because it makes an
external signal look like a workflow action, exposes no clear execution owner,
and does not distinguish an intentional adapter shim from a misconfigured
transition with no authorized roles.
