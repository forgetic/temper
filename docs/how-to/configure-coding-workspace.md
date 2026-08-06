# Configure the coding agent (real-implementation PRs)

Use this when a workflow role should open implementation PRs from real code
changes rather than from synthetic branch prep. The worker prepares a git
checkout, spawns the out-of-process coding agent (`temper agent`) against it, and
turns the agent's diff or verdict into a PR or a routed workflow outcome.

There is no longer any `TEMPER_CODING_WORKSPACE_*` "binding" env var, and no
`coding_workspace` external-tool declaration to add. The mechanism is entirely
config-driven: the **workflow** declares the action the role performs, the
**daemon** derives the checkout capability and verdict vocabulary from that
action, and the **config/credentials files** wire up the provider the worker
renders onto the `temper agent` command line. This page walks that path.

## 1. Let the workflow declare the implementing action

A role gets a real coding turn whenever the daemon assigns it a
**workspace-backed action** — an action that either declares a
`create_pull_request` effect (the head path, e.g. an engineer `open_pr`) or
declares verdict `outcomes` (the verdict path, e.g. a reviewer `approve` /
`request_changes`). No external-tool declaration is needed; the role's compiled
tools and the queue assignment are enough.

```json
{
  "id": "engineer",
  "prompt": {
    "guidance": "Only report success after producing a real implementation diff.",
    "tool_guidance": "Do not open bookkeeping-only PRs."
  },
  "queues": ["code_ready"]
}
```

For each assigned job the daemon enriches the worker's job context with:

- `action` — the assigned action name.
- `allowed_verdicts` — the verdict vocabulary the action declares (the keys of
  its compiled `outcomes` map). This is the **only** set of verdicts the agent
  may write to the result file (§3); emitting anything else fails the tick. The
  array is empty for a pure head action that declares no `outcomes`. The
  reference engineer `open_pr` action declares decline outcomes such as
  `needs_architect` and `needs_human`; omitting `verdict` still takes the normal
  PR head path.
- `checkout` — the checkout capability for this action (see the table in §2).

A queue action may pin the checkout explicitly with a `checkout` field on the
queue's action assignment; otherwise the daemon infers it: an action with a
`create_pull_request` effect ⇒ `writable`; an Issue-targeted verdict action ⇒
`read_only`; a PR-targeted verdict action ⇒ `pull_request_read_only`. The
workflow stays the single source of truth for the action's options, so a bound
agent should read `allowed_verdicts` and constrain itself to that set rather than
inventing a verdict.

## 2. Configure the provider the worker renders onto `temper agent`

`temper-worker` no longer binds an external provider from env. It spawns the
in-tree coding agent (`temper agent`) once per job, reading the
provider/model/iteration knobs from the resolved deployment config and rendering
them as **command-line flags**. Exactly one secret — the provider credential —
crosses as an environment variable (`TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`),
which the worker builds from the credentials file and injects into the child.

Put non-secret wiring in the config file (`temper.toml`; the on-disk default
filename is `config.toml`) and the provider secret in the credentials file
(`credentials.toml`). Top-level `temper init` writes a complete deployment
bundle for onboarding; the legacy `temper config init` template writer remains
available only for scripts that need bare copy-pasteable TOML templates.

```toml
# temper.toml — non-secret deployment settings

[engine]
repos = ["acme/widgets"]
roles = ["architect", "engineer", "code-reviewer"]

[worker]
# Top-level directory under which per-job checkouts are prepared. Each job is
# isolated below this root as <role>/<safe-coordination-key>/<repo-dir>.
# Default: $XDG_STATE_HOME/temper/workspace (~/.local/state/temper/workspace).
# workspace = "~/.local/state/temper/workspace"
# Explicit owner/name:role capabilities this worker serves.
# Default: the cross-product of [engine] repos × roles.
# capabilities = ["acme/widgets:engineer"]
# Worker-owned liveness supervision. Lease heartbeats are not agent progress.
max_no_progress_secs = 900
# max_run_secs = 7200       # optional independent whole-run ceiling
graceful_cancellation_grace_secs = 10
forced_termination_grace_secs = 5

[agent]
provider = "anthropic"          # selects the [agent.providers.<name>] block below
max_iterations = 250
enable_subagents = false        # the investigate read-only sub-agent tool

[agent.deadlines]
# Complete defaults are 600/120/120 seconds when this table is absent.
tool_timeout_secs = 600
model_connect_timeout_secs = 120
model_idle_timeout_secs = 120

[agent.providers.anthropic]
# url = "https://api.anthropic.com"   # optional base-URL override → --provider-url
models = { main = "claude-opus-4-8", investigate = "claude-haiku-4-5" }
# A codebase-memory provider is optional. Temper requires provider 0.9.0+ for
# targeted stable identity and host-only bounded maintenance.
[agent.tools.codebase_memory]
mode = "auto"
command = "codebase-memory-mcp"
args = []
roles = ["architect", "engineer", "code-reviewer"]
index = "background"
startup_timeout_secs = 5
index_timeout_secs = 120

[agent.tools.codebase_memory.retention]
enabled = true
max_obsolete_projects = 64
max_age_days = 30
maintenance_interval_secs = 3600
maintenance_timeout_secs = 30
inventory_page_size = 50
max_inventory_pages = 20
max_deletions_per_run = 16
```

Codebase-memory indexes use a stable key derived from the configured logical
Forge repository, not the prepared checkout path. Runtime workers use the
retention settings above for ownership-safe bounded maintenance. Operators use
the same resolved provider, workspace scope, classifier, and limits through the
dry-run-first `temper maintenance codebase-memory` command. Follow
[Recover a codebase-memory cache safely](recover-codebase-memory.md) before any
apply or stable-project rebuild; do not delete the provider cache directory.

```toml
# credentials.toml — secrets (chmod 600, keep out of version control)

# LLM provider secret, keyed to match [agent.providers.<name>] in temper.toml.
[agent.providers.anthropic]
type = "oauth"                  # "oauth" (access/refresh/expires) or "api-key" (key)
access = "<oauth-access-token>"
refresh = "<oauth-refresh-token>"
expires = 0                     # access-token expiry, unix milliseconds
# Or point at an existing pi-format auth.json instead of inline OAuth fields:
# auth_file = "/home/agent/.pi/agent/auth.json"

# A static-key provider (DeepSeek / OpenAI-compatible) would instead be:
# [agent.providers.deepseek]
# type = "api-key"
# key = "<deepseek-api-key>"
```

From this resolved config the worker renders the agent command. The non-secret
provider knobs become flags — `--provider <kind>`, and `--model`,
`--investigate-model`, `--provider-url` when set — alongside `--max-iterations`,
`--subagents on|off`, and a per-job `--context`, `--result`, and `--workspace`.
For known first-party commands it also writes the complete resolved operation
limits to a private per-run JSON file, passes `--runtime-limits <FILE>`, and
passes an attempt-owned `--agent-lifecycle-address <HOST:PORT>`. The lifecycle
channel carries only bounded model/tool/steering/end boundaries and remains on
when optional activity trace capture or storage is unavailable.
Each `[agent.profiles.<name>.deadlines]` field inherits independently from
`[agent.deadlines]`. Explicit third-party profile commands receive neither this
Temper-specific flag nor first-party lifecycle flags; worker fallback supervision
still applies to them. The credential becomes the single
`TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON` env
the child reads. (For the full flag inventory and defaults, see
[`docs/reference/environment-variables.md`](../reference/environment-variables.md).)

The agent runs **in the prepared coordination-scoped workspace root** (its
`--workspace`, also its cwd), with each manifest repo checked out below that root
using its `dir`. It receives the assigned job, work item, user guidance, action,
allowed verdicts, branch/base/correlation data, and the optional versioned
`artifact_context` bundle as fields of the `WorkspaceContext` JSON the worker writes to the
`--context` file. The legacy singular work-item JSON remains present for older
agents. When configured, the worker also exposes assignment-bound
`forge_get_item` and `forge_list_related` tools over a per-run loopback channel;
the agent supplies only a bounded read operation, while worker/job identity and
authentication stay on the host side. The agent does
**not** receive Forge credentials or mutation tools; it completes the assigned action and
reports a branch/diff, a declared verdict with authored content, or a structured
failure for the worker to return to Temper. See the
[Worker/Agent process protocol](../reference/worker-agent-process-protocol.md)
for exact channel shapes, limits, and errors.

### Assigned-action checkout capability

Different assigned actions need different checkouts, so the daemon derives a
checkout capability (§1) and the worker prepares the checkout to match. The
capability is surfaced to the agent as the `WorkspaceContext.checkout` field (the
worker also lays the repos out so this is reflected in each repo's `access` and,
for writable repos, its `branch_hint`):

| Capability | `checkout` value | Checkout behavior |
| --- | --- | --- |
| Writable issue implementation | `writable` | Writable checkout at `base`; the head path commits and the worker pushes the work branch (the repo's `branch_hint`) for a new PR. |
| Read-only issue verdict | `read_only` | Read-only checkout at `base`; the agent must route a verdict and never commits. |
| Read-only PR verdict/review | `pull_request_read_only` | Read-only checkout with the PR head fetched **and** base, so the agent can compute `git diff <base> <pr-head>`; never commits. |
| Writable PR-head fix | `pull_request_writable` | Writable checkout on the existing PR head branch; the worker pushes fixes back to that branch so CI/review gates can re-evaluate. |

A read-only command (`read_only` or `pull_request_read_only`) **must** write a
`verdict` to the result file (§3): producing a diff without a verdict in a
read-only checkout is a misconfiguration and the worker fails the job loudly
rather than committing into a tree the operator declared read-only. The
reviewer's CI status is summarized into the action guidance the worker assembles
for the PR-fix path; richer CI enrichment is a follow-up.

## 3. Report a result (verdict / content)

The agent writes its terminal work product to the file named by the `--result`
flag: a single `WorkspaceResult` JSON object. Writing it is optional for the head
path — leave it untouched (or write an empty/`verdict`-less object) to keep the
default behavior. Every field is optional:

```jsonc
{
  "verdict": "ready_code",          // optional; omit/null ⇒ default head path
  "summary": "one-line summary",    // optional; falls back to changed-files list
  "body": "rewritten issue body",   // optional; consumed by a routed set_body
  "review_body": "review prose",    // optional; consumed by a routed attach_review
  "labels": ["implementation"],     // optional; PR labels for the head path
  "children": [                     // optional; consumed by a routed create_issues
    {
      "slug": "api",                // stable id; seeds the child's correlation key
      "title": "Add the HTTP API",  // authored child title
      "body": "…",                  // authored child body
      "labels": ["code", "ready"],  // labels to create the child with
      "depends_on": [],             // slugs of sibling children that must land first
      "target_repo": "acme/widgets" // optional owner/name; omit ⇒ parent's repo
    }
  ]
}
```

Unknown keys are ignored by the wire parser for forward compatibility, so agents
should stick to the documented fields above; unsupported fields such as legacy
`plan` data are not used to render PR progress. Each `children` entry is one
dependent child artifact: when the verdict routes to a transition that declares a
`create_issues` effect (e.g. an architect `needs_breakdown` verdict routing an
intake to a breakdown transition), the provider hands the authored children to
that effect. Children land idempotently under a deterministic per-effect key,
each linked back to the routed artifact as parent, and a child naming a sibling
slug in `depends_on` records that dependency once both exist. A child's
`target_repo` selects which repo it is created in, defaulting to the parent's.
`children` is ignored on the head path and when the routed transition declares no
`create_issues`.

The worker chooses one of two paths based on whether `verdict` is present:

- **No result file / empty file / no `verdict`** → the **head path**. The worker
  enforces the diff check (§4), commits, and pushes the work branch; the daemon
  then opens the PR. A `labels` or `summary` value in the result file overrides
  the default PR labels and the default changed-files summary; the PR is still
  opened from the committed diff.
- **`verdict` present** → the **verdict path**. The verdict must be one of the
  action's declared `allowed_verdicts` (surfaced in the context, §1); an
  out-of-vocabulary verdict is rejected — naming the declared set — rather than
  handing a doomed verdict to the runner. The worker **skips** the diff check,
  the commit, and the push, and tolerates an empty working tree. It returns a
  verdict-only output (empty branch) that routes through the action's declared
  `outcomes` map, carrying any `body` (for a routed `set_body`) and `review_body`
  (for a routed `attach_review`). This is how the agent emits e.g. an engineer
  `needs_architect` / `needs_human` decline with no implementation PR, a reviewer
  `approve` with no diff, or an architect rewrite.

`changed_files` is always computed by the worker from `git status` on the head
path; the agent never reports it.

The agent does not publish mid-run control-plane state. Operators should treat
the terminal `WorkspaceResult` plus worker/daemon logs as the durable job record;
PR bodies render the final summary and workflow metadata rather than
model-authored progress checklists.

## 4. Safety behavior

The head path must leave a real diff: the worker rejects a job that produced no
tree changes in any writable repo (and, for a PR-head fix, a job that did not
change the PR head — "CI stays red") with a permanent failure rather than opening
an empty PR. The worker commits and pushes only the work branch; the **daemon**
opens the PR from that branch outcome, so Forge/workflow mutation stays behind
the normal authority boundary and the token-holding agent never opens PRs itself.

The Smith-owned dogfood launcher keeps `DOGFOOD_ENABLE_ENGINEER_AUTOMATION=0`
by default. Turning it on requires the config above (a configured
`[agent.providers.*]` provider plus its credentials), an engineer role whose
workflow declares a writable implementing action, active PR diff-guard settings,
and forge role credentials. Check that live setup from the Smith checkout:

```sh
cd ~/src/rust/smith/examples/dogfood
./run.sh preflight
```

Only set `DOGFOOD_ENABLE_ENGINEER_AUTOMATION=1` for an intentional live issue
whose produced diff passes the guard.
