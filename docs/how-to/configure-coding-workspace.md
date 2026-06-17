# Configure a coding workspace external tool

Use this when a workflow role should open implementation PRs from real code
changes rather than from synthetic branch prep.

## 1. Declare the tool in the workflow

Add an external tool declaration to the role that is allowed to implement code.
The conventional id is `coding_workspace`:

```json
{
  "id": "engineer",
  "prompt": {
    "guidance": "Use open_pr only after coding_workspace produced a real implementation diff.",
    "tool_guidance": "Do not open bookkeeping-only PRs."
  },
  "external_tools": [{
    "id": "coding_workspace",
    "description": "Prepare a checkout, edit code, commit a PR head, and report changed files.",
    "required": false,
    "constraints": ["Only touch the checked-out repository workspace."],
    "guidance": "If unbound, choose no_action for implementation work."
  }],
  "queues": ["code_ready"]
}
```

Set `required: true` only when every runner that builds the compiled workflow is
expected to have a binding. Optional declarations are omitted from the runtime
prompt/context until a runner binds them.

## 2. Bind the production local-git provider

`temper-worker` binds the local-git workspace provider when these environment
variables are present:

```sh
export TEMPER_CODING_WORKSPACE_ROOT=/path/to/clean/checkout
export TEMPER_CODING_WORKSPACE_COMMAND='your-coder --context "$TEMPER_CODING_WORKSPACE_CONTEXT"'
export TEMPER_CODING_WORKSPACE_REMOTE=origin        # default
export TEMPER_CODING_WORKSPACE_PUSH=1              # default; set 0 for local tests
export TEMPER_CODING_WORKSPACE_PR_LABELS=implementation,needs-reviewer
```

The worker binds the provider for **every declared workspace external-tool id**,
not just `coding_workspace`. Any role-declared id equal to `coding_workspace` or
ending in `_workspace` (e.g. `triage_workspace`, `review_workspace`) is bound to
this provider, deduped per `(role, id)`. Non-workspace external tools are left
unbound for a different provider. When the env above is set but no role declares
any workspace id, the worker logs a diagnostic and binds nothing.

The command runs in the checkout. It receives the work item, assigned workflow
action, and user guidance in `TEMPER_CODING_WORKSPACE_CONTEXT` (JSON), plus
branch/base/correlation env vars. The LLM does not receive Forge mutation tools;
it must perform the assigned action and return a branch/diff, declared verdict,
or structured failure for Temper to validate and apply.

The context JSON includes an `action` string and an `allowed_verdicts` array: the
verdict vocabulary the action declares (the keys of its compiled `outcomes` map).
This is the **only** set of verdicts the command may write to the result file (§3) — emitting anything
else fails the tick. A bound agent should read `allowed_verdicts` and constrain
itself to that set rather than guessing a verdict, so the workflow stays the
single source of truth for the action's options. The array is empty for a pure
head action that declares no `outcomes` (the engineer `open_pr` default), where
no verdict is expected at all.

### Assigned-action checkout capability

Different assigned actions need different checkouts, so the worker receives a
checkout capability derived from the selected action and, when necessary, the
queue's explicit action assignment (never from a hard-coded role id). The
provider receives the capability per invocation and exposes it to the command as
`TEMPER_CODING_WORKSPACE_CHECKOUT`:

| Capability | `…CHECKOUT` | Checkout behavior |
| --- | --- | --- |
| Writable issue implementation | `writable` | Writable checkout at `base`; the head path commits and pushes a branch for a new PR. |
| Read-only issue verdict | `read_only` | Read-only checkout at `base`; the command must route a verdict and never commits. |
| Read-only PR verdict/review | `pull_request_read_only` | Read-only checkout with the PR head fetched (`TEMPER_CODING_WORKSPACE_PR_HEAD_REF`) **and** base so the command can compute `git diff <base> <pr-head-ref>`; never commits. |
| Writable PR-head fix | `pull_request_writable` | Writable checkout on the existing PR head branch; the worker pushes fixes back to that branch so CI/review gates can re-evaluate. |

A read-only command (`read_only` or `pull_request_read_only`) **must** write a
`verdict` to the result file (§3): producing a diff without a verdict in a
read-only checkout is a misconfiguration and the provider fails loudly rather
than committing into a tree the operator declared read-only. The reviewer's CI
status is read from the granted work-item context; richer CI enrichment of that
context is a follow-up.

## 3. Report a result (verdict / content)

The command also receives `TEMPER_CODING_WORKSPACE_RESULT`: a path to a
provider-created result file the command **may** write to report a verdict and/or
content back to the workspace. Writing it is optional — leave it untouched to keep
the default behavior. When written, it is a single JSON object; every field is
optional:

```jsonc
{
  "verdict": "ready_code",          // optional; omit/null ⇒ default head path
  "summary": "one-line summary",    // optional; falls back to changed-files list
  "body": "rewritten issue body",   // optional; consumed by a routed set_body
  "review_body": "review prose",    // optional; consumed by a routed attach_review
  "labels": ["implementation"],     // optional; overrides default PR labels (head path)
  "children": [                     // optional; consumed by a routed create_issues
    {
      "slug": "api",                // stable id; seeds the child's correlation key
      "title": "Add the HTTP API",  // authored child title
      "body": "…",                  // authored child body
      "labels": ["code", "ready"],  // labels to create the child with
      "depends_on": []              // slugs of sibling children that must land first
    }
  ]
}
```

Unknown keys are rejected, so a typo fails loudly rather than being silently
ignored. Each `children` entry is one dependent child artifact: when the verdict
routes to a transition that declares a `create_issues` effect (e.g. an architect
`needs_breakdown` verdict routing an intake to a breakdown transition), the
provider hands the authored children to that effect. Children land idempotently
under a deterministic per-effect key, each linked back to the routed artifact as
parent, and a child naming a sibling slug in `depends_on` records that dependency
once both exist. `children` is ignored on the head path and when the routed
transition declares no `create_issues`.

The provider chooses one of two paths based on whether `verdict` is present:

- **No result file / empty file / no `verdict`** → the **head path**. The
  provider enforces the diff guard (§4), commits, pushes the branch, and opens a
  PR exactly as before. A `labels` or `summary` value in the result file
  overrides the configured PR labels and the default changed-files summary; the
  PR is still opened from the committed diff.
- **`verdict` present** → the **verdict path**. The verdict must be one of the
  action's declared `allowed_verdicts` (surfaced in the context, §2); the provider
  rejects an out-of-vocabulary verdict here, naming the declared set, rather than
  handing a doomed verdict to the runner. The provider **skips** the diff
  guard, the commit, and the push, and tolerates an empty working tree. It
  returns a verdict-only output (empty branch) that routes through the action's
  declared `outcomes` map, carrying any `body` (for a routed `set_body`) and
  `review_body` (for a routed `attach_review`). This is how an external command
  emits e.g. a reviewer `approve` with no diff, or an architect rewrite.

`changed_files` is always computed by the provider from `git status` on the head
path; the command never reports it.

## 4. Safety behavior

The workspace must leave a real non-bookkeeping diff. Production rejects empty or
synthetic-only changes such as `.temper-pr-prep/`, `.temper-ci/`, or the demo CI
workflow file. The worker then commits and pushes the branch and opens the PR via
`RoleTools`, so Forge/workflow mutation stays behind the normal authority
boundary.

The Smith-owned dogfood launcher keeps `DOGFOOD_ENABLE_ENGINEER_AUTOMATION=0`
by default. Turning it on requires the workspace env above, the engineer role's
`coding_workspace` declaration, active PR diff guard settings, and role
credentials. Check that live setup from the Smith checkout:

```sh
cd ~/src/rust/smith/examples/dogfood
./run.sh preflight
```

Only set `DOGFOOD_ENABLE_ENGINEER_AUTOMATION=1` for an intentional live issue
whose produced diff passes the guard.
