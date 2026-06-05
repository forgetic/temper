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
| `artifact_kind` | Logical item mapped to a Forge target (`issue` or `pull_request`) plus identifying labels. |
| `state_dimension` | Named state group projected as labels. Dimensions are exclusive by default; states may restrict legal artifact kinds. |
| `queue` | Query over artifact kind(s), labels, optional disjunctive label branches, optional runtime/projected condition, activation policy, and optional automation metadata. |
| `transition` | Guarded action authorized for roles. Effects may update labels/assignees, create comments, create PRs, request reviewers, submit reviews, or merge PRs. |
| `gate` | Condition that unlocks a transition from projected labels/state, sibling transition outcomes, or runtime signals such as dependencies, CI, and reviews. |
| `relation` | Typed link between artifacts: `parent`, `dependency`, or `produced_pr`. |
| `invariant` / `recovery_policy` | Reserved concepts; not first-class implemented spec primitives yet. |

Labels are the portable projection for workflow-owned state. Native dependency
links, CI jobs, review decisions, and merge state are observed from the Forge
rather than mirrored as labels. Metadata blocks carry information with no
portable Forge field: kind overrides, parent links, fallback/cross-repo
dependencies, correlation keys, and leases.

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
state and Forge mutation still happen through `RoleTools` and the executor.

## Static validation

Validation rejects or diagnoses:

- duplicate ids and unknown serde fields;
- empty queue artifact-kind lists;
- references to undeclared roles, labels, artifact kinds, states, queues,
  transitions, gates, relations, or external tools;
- artifact/state mismatches, including labels illegal for an artifact kind;
- queue automation whose actor/transition/fallback is missing, unauthorized, or
  incompatible with the queue's artifact kinds.

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
- `QueueManifest` entries with subscribers, filters, activation policy, and
  optional automation metadata;
- `TransitionManifest` entries forming the runtime transition table;
- `LabelManifest` / `LabelSpec` / `LabelUsage` for Forge label provisioning.

Generated prompt prose is deterministic and role-id agnostic. User-specific
judgment stays in the role prompt fields or fixtures; production Temper does not
hard-code reference-delivery prompts. Generated tools expose named workflow
intents such as `claim_code`, not generic Forge mutation operations.
