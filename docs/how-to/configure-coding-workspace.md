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
export TEMPER_CODING_WORKSPACE_PR_LABELS=implementation,needs-reviewer,needs-merge
```

The command runs in the checkout. It receives the work item and user guidance in
`TEMPER_CODING_WORKSPACE_CONTEXT` (JSON), plus branch/base/correlation env vars.
The LLM does not receive shell or file tools; it can only choose the authorized
workflow action, after which the runner invokes this configured provider.

## 3. Safety behavior

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
