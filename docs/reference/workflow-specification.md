# Workflow specification and compilation

This page defines the declarative workflow model that `temper-workflow` accepts
and compiles. For runtime behavior, read [workflow-runtime.md](workflow-runtime.md).

## Type phases

- `RawWorkflowSpec` and raw child structs load from YAML, JSON, TOML, or
  generated input.
- `ValidatedWorkflow` is the normalized model. It has no public constructor; use
  `RawWorkflowSpec::validate` / `validate::validate`.
- `CompiledWorkflow` is the runner-facing projection: role, tool, queue,
  transition, prompt, and label manifests.
- Runtime APIs operate on validated or compiled workflows plus backend handles.

Validation errors are diagnostic collections so users can fix several spec
issues in one pass. Ids are typed (`RoleId`, `QueueId`, `TransitionId`,
`GateId`, `ArtifactKindId`, `StateDimensionId`, `StateId`, `LabelId`,
`ExternalToolId`) rather than raw strings at public boundaries.

## Spec primitives

| Primitive | Meaning |
| --- | --- |
| `role` | Actor authority, subscribed queues, concurrency hint, prompt guidance, and declared non-workflow external tools. |
| `artifact_kind` | Logical item mapped to a Forge target (`issue` or `pull_request`) plus identifying labels and optional initial creation labels. |
| `state_dimension` | Named state group projected as labels. Dimensions are exclusive by default; states may restrict legal artifact kinds. |
| `queue` | Query over artifact kind(s), required labels, excluded labels, optional disjunctive label branches, optional runtime/projected condition, activation policy, optional role-worker action assignments, and optional automation metadata. |
| `transition` | Guarded action authorized for roles. Effects may update labels/assignees, create comments, create PRs, request reviewers, submit reviews, or merge PRs. `remove_label` normally requires the label to be present; `"if_present": true` makes it a no-op cleanup when absent. |
| `gate` | Condition that unlocks a transition from projected labels/state, sibling transition outcomes, or runtime signals such as dependencies, CI, and reviews. |
| `relation` | Typed link between artifacts: `parent`, `dependency`, or `produced_pr`. |
| `invariant` / `recovery_policy` | Reserved concepts; not first-class implemented spec primitives yet. |

Labels are the portable projection for workflow-owned state. Native dependency
links, CI jobs, review decisions, and merge state are observed from the Forge
rather than mirrored as labels. Metadata blocks carry information with no
portable Forge field: kind overrides, parent links, fallback/cross-repo
dependencies, correlation keys, and leases.

## Artifact kinds

An `artifact_kind` maps a logical workflow item to one Forge target (`issue` or
`pull_request`) and declares the labels that classify existing Forge artifacts:

- `identifying_labels` are part of the artifact kind's identity. A Forge artifact
  is classified as the kind only when every identifying label is present, and
  daemon correlation lookups use these stable labels to find already-created
  artifacts.
- `initial_labels` are creation-time labels the engine adds in addition to the
  identifying labels when it creates an artifact of this kind. They are useful
  for initial routing, such as putting a newly opened implementation PR into a
  reviewer queue. They are not kind identity, so later transitions may freely
  remove them (for example after review starts or completes) without breaking
  idempotent correlation lookup.

Do not use another artifact kind's identifying label as an `initial_labels` entry:
that can make a newly created artifact classify as the wrong kind or as multiple
kinds until the routing label is removed.

## Roles, prompts, and external tools

A role's `charter`, `prompt.guidance`, and `prompt.tool_guidance` are
user-authored guidance. They never grant Forge permission or workflow authority.
Authority comes only from transitions that authorize that role.

`external_tools` declarations describe desired non-workflow tools (`id`,
`description`, optional `required`, `constraints`, and `guidance`). A runner must
bind a matching provider before exposing a declared tool. Required unbound tools
fail worker preflight; optional unbound tools are omitted or marked unavailable;
undeclared bindings are rejected.

The conventional `coding_workspace` provider prepares a checkout and branch for
engineering work. It feeds `CreatePullRequest` runtime context, but workflow
state and Forge mutation still happen through `RoleTools` and the executor. A
`create_pull_request` effect may optionally declare `artifact_kind`; when set,
validation requires that kind to exist and target `pull_request`, allowing
verdict-driven runtimes to derive labels and metadata for PR artifacts such as a
feature-branch landing PR without requiring a worker-produced diff.

A `create_issues` effect creates one or more child issues from the workspace
result. It accepts `min_children` (default `1`) and optional `max_children` to
declare product cardinality; `max_children: 1` expresses an exact one-child
handoff. Validation rejects a zero minimum or a maximum below the minimum.
`required_child_metadata` optionally lists non-blank workflow metadata that
every authored child body must carry; the currently supported key is
`target_branch`. This requirement is included in the worker/agent verdict
contract and revalidated by the engine before any child or normal workflow
state is mutated. The effect also accepts an optional `correlation_key` for
idempotent child creation and an optional `record_parent_dependencies` boolean.
The default `false` preserves legacy same-repository fan-out behavior. When set to `true`, the
executor records every created child as dependency metadata on the source issue
after all children exist and sibling dependency slugs have been linked. Use this
for plan-completion issues whose `dependencies_resolved` gate should remain
blocked until their architect-created code/validation children close or land.

## Queue filters

`labels` are conjunctive required labels; `excluded_labels` are labels that must
be absent for a queue to match. Use `excluded_labels` for temporary blockers
that should pause a durable handoff queue without removing the handoff label
itself (for example, keep `landing` while `merge-conflict` routes repair work).

## Queue role-worker actions

A queue may declare `actions` entries that bind matched role work to a concrete
workflow transition/action:

```json
{
  "id": "pr_ci_failed",
  "artifact": "implementation_pr",
  "condition": { "kind": "ci_failed" },
  "actions": [
    {
      "role": "engineer",
      "action": "address_ci_failure",
      "checkout": "pull_request_writable"
    }
  ]
}
```

`role` names the subscribed worker role, `action` names an authorized transition,
and optional `artifact` disambiguates multi-kind queues. Optional `checkout`
selects the worker checkout capability when the transition shape alone is not
enough (notably PR-head fix queues). Optional `guidance` is appended to generated
job guidance. Temper validates that the referenced role/action/artifact exist,
that the action authorizes the role and operates on a queue-selected artifact,
and that checkout capability tokens are supported.

## Static validation

Validation rejects or diagnoses:

- duplicate ids and unknown serde fields;
- empty queue artifact-kind lists;
- references to undeclared roles, labels, artifact kinds, states, queues,
  transitions, gates, relations, or external tools;
- artifact/state mismatches, including labels illegal for an artifact kind;
- queue role-worker actions whose role/action/artifact is missing, unauthorized,
  incompatible with the queue's artifact kinds, or declares an unsupported
  checkout capability;
- queue automation whose actor/transition/fallback is missing, unauthorized, or
  incompatible with the queue's artifact kinds;
- `create_pull_request.artifact_kind` values that are undeclared or name a
  non-`pull_request` artifact kind.

Planned checks include contradictory transition effects, unsatisfiable gates,
role tool declarations that exceed transition authority, unrepresentable Forge
artifact mappings, unreachable queues, unexplained terminal states, and unused
labels.

## Compilation outputs

`compile::compile` and `ValidatedWorkflow::compile` are infallible because they
consume a validated workflow. The compiled model contains:

- `RoleManifest` per role, with prompt sections, concurrency, subscribed queues,
  transition authority, declared external tools, and workflow tools;
- `ToolManifest` entries, one intent-level operation per authorized transition;
- `QueueManifest` entries with subscribers, filters, activation policy,
  role-worker action assignments, and optional automation metadata;
- `TransitionManifest` entries forming the runtime transition table;
- `LabelManifest` / `LabelSpec` / `LabelUsage` for Forge label provisioning.

Generated prompt prose is deterministic and role-id agnostic. User-specific
judgment stays in the role prompt fields or fixtures; production Temper does not
hard-code reference-delivery prompts. Generated tools expose named workflow
intents such as `claim_code`, not generic Forge mutation operations.
