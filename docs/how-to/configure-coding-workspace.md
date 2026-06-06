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

`temper-worker` binds `coding_workspace` when these environment variables are
present:

```sh
export TEMPER_CODING_WORKSPACE_ROOT=/path/to/clean/checkout
export TEMPER_CODING_WORKSPACE_COMMAND='your-coder --context "$TEMPER_CODING_WORKSPACE_CONTEXT"'
export TEMPER_CODING_WORKSPACE_REMOTE=origin        # default
export TEMPER_CODING_WORKSPACE_PUSH=1              # default; set 0 for local tests
export TEMPER_CODING_WORKSPACE_PR_LABELS=implementation,needs-reviewer
```

The command runs in the checkout. It receives the work item and user guidance in
`TEMPER_CODING_WORKSPACE_CONTEXT` (JSON), plus branch/base/correlation env vars.
The LLM does not receive shell or file tools; it can only choose the authorized
workflow action, after which the runner invokes this configured provider.

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
  "children": []                    // optional; reserved for dependent children
}
```

Unknown keys are rejected, so a typo fails loudly rather than being silently
ignored.

The provider chooses one of two paths based on whether `verdict` is present:

- **No result file / empty file / no `verdict`** → the **head path**. The
  provider enforces the diff guard (§4), commits, pushes the branch, and opens a
  PR exactly as before. A `labels` or `summary` value in the result file
  overrides the configured PR labels and the default changed-files summary; the
  PR is still opened from the committed diff.
- **`verdict` present** → the **verdict path**. The provider **skips** the diff
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
