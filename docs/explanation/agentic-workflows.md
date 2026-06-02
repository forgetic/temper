# Agentic workflows

Temper workflows coordinate disposable agents through durable Forge artifacts.
The Forge layer stores issues, pull requests, comments, labels, reviews, CI jobs, dependency links, and merges. The workflow layer interprets those artifacts as a state machine. The agent layer is compiled from the workflow and receives prompts plus a narrow set of role-specific tools.

## Layer model

The intended stack is:

1. `temper-forge`: provider-neutral collaboration interface.
2. `temper-workflow`: workflow specifications, validation, compilation, runtime transitions, and recovery.
3. agent runners: LLM or human workers using generated prompts and tools.

This separation keeps provider details out of workflow policy and keeps generated agent prompts focused on workflow mechanics and the authority each role actually has. Role behavior is user configuration: a role may provide prompt guidance, tool guidance, and external-tool declarations, but prose and declarations do not create executable tools or permissions until the runner binds matching providers. For where these layers run as processes once a real backend exists, see [End-to-end big picture](end-to-end-big-picture.md).

## Workflow concepts

An artifact is a logical work item mapped to a Forge issue or pull request. Examples are `epic`, `design`, `code`, and `implementation_pr`.

A state dimension is a named group of states, often projected as labels. For example, a code issue lifecycle may contain `ready`, `blocked`, and `in-progress`. Review outcomes are native Forge state instead of workflow-owned labels; a workflow can still use a `needs-reviewer` routing label while gates read native review decisions.

A queue is a query over artifacts, such as `code + ready` issues or PRs whose native CI status is failed.

A transition is an authorized state change with preconditions and effects. Generated tools should map to transitions instead of exposing generic label mutation.

A role prompt has generated mechanics from the workflow compiler plus optional user-authored guidance from the workflow spec. The generated part is role-id agnostic; it lists identity, context, subscribed queues, authorized workflow actions, and the rule that external tools are available only when both declared by the workflow and bound by the runner. Judgment such as how to implement, review, or escalate belongs in user guidance, not in checked-in production prompt files or role-specific adapters. Engineering-style roles use the declared `coding_workspace` external tool for product-code diffs; the LLM does not get broad shell/file tools, and PR creation still goes through `RoleTools` with the workspace-produced branch.

A gate is a condition that unlocks a transition. For example, a PR may be mergeable only when native CI and review gates both succeed. A gate may be satisfied by a workflow transition, by Forge-projected labels/state, or by a native signal such as CI job conclusions or dependency target state.

A relation links artifacts, such as feature request to epic, epic to design issue, design issue to code issue, and code issue to PR. Relations can cross repository boundaries through repo-qualified artifact references; see [Cross-repository workflow model](cross-repo-workflows.md).

## Example delivery workflow

A worked, cleaned-up version of this example — the target the spec, planner, and
executor should express and run — is in
[Reference delivery workflow](reference-workflow.md).

A human files a high-level request. The architect turns it into one or more epics and design issues. If long-term direction is unclear, the architect labels the design issue `needs-owner`.

The owner comments on issues requiring owner input and periodically reviews landed PRs for alignment with project values. If owner judgment is insufficient, the owner switches the design issue to `needs-human` for the explicit human role.

Once a design is ready, the architect creates code issues. Some are labeled `ready`; others are `blocked` until dependencies land.

Engineers claim `code + ready` issues, changing them to `in-progress`, implement the work, and open PRs labeled for review; CI status is read from the Forge.

CI and review gates proceed independently. Reviewers are requested on the PR and submit native review decisions; the workflow reads that native aggregate and the current CI jobs. If all gates pass, the PR can merge. If either gate fails, the work returns to the engineer or escalates according to workflow policy.

## Robustness model

Agents are disposable. They may crash, repeat calls, lose context, or resume after another worker has changed the same artifact. The workflow runtime is the authority that checks fresh Forge state before every transition.

The Forge projection should remain understandable to humans: labels and comments show workflow-owned public state. Native Forge dependency links and pull-request review state store dependency and review facts; machine-readable metadata in bodies or comments can store workflow kind, parent links, fallback dependency links, correlation keys, and leases. Temper stores this as a JSON block inside an HTML comment so it renders invisibly while staying deterministic to parse; see the metadata block format in `docs/reference/workflow-layer.md`.

The workflow layer reads labels plus that metadata block to classify a Forge issue or pull request into a typed artifact. Classification detects impossible label combinations (for example, two states of one mutually exclusive dimension) and label/metadata drift, so the reconciler and operator queues have a precise picture of state before any transition runs.

Claims should be leases with expiration. If an engineer crashes while an issue is `in-progress`, the reconciler can detect the expired lease and return the issue to `ready`, escalate it, or leave a diagnostic comment.

Create operations should be idempotent. A tool that creates a PR for an issue should first look for an existing PR with the same correlation key and return it instead of creating a duplicate.

## Triggering model

The runtime is pull-based: queues are queries and the executor re-loads fresh
Forge state before every transition. Triggering decides *when* to run a queue
scan; it is deliberately not part of the `temper-forge` trait, which stays a
request/response query+mutation contract.

The intended model is level-triggered with an edge-triggered accelerator, the
Kubernetes-controller pattern:

- Periodic polling is level-triggered: authoritative, lossless, and the liveness
  backstop. It is mandatory, not optional.
- Webhooks (Forgejo, in the real backend) are edge-triggered hints that lower
  latency between one agent delivering work and the next reacting. They are
  lossy and must be treated as a signal to pull, never as a source of truth.

Both feed the same reaction path (pull → classify → plan → execute → reconcile).
Webhook receipt, verification, and payload parsing are provider-specific and
live in the backend/runner layer. The portable `ChangeHint` and `ChangeSource`
companion types carry push-style wakeups without adding notification methods to
the `Forge` trait. See ADR 0009 for the decision.

## Rust's role

Rust should make invalid internal states hard to express. Runtime code should accept only validated workflows, handle workflow effects exhaustively, and use typed identifiers instead of raw strings.

Rust cannot prove external Forge state is valid. Humans and providers can change labels, comments, and PR state outside the runtime. The reconciler therefore remains a core workflow component, not an optional cleanup task.
